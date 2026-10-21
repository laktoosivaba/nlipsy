//! `export interface engine` — ECMA-430 clause 6, and the composed order around it.
//!
//! Free functions, all of them. Nothing here holds state between calls and nothing here
//! is a resource: the [`Conformity`](nlipsy_core::Conformity) each one runs under is
//! built per call from `host-endpoint.conformity`, `host-endpoint.rejection-policy` and
//! the `host-authority` import (`crate::host::steps`, `crate::host::composed`), so
//! nothing needs to be `'static` that need not be.
//!
//! The **order** is `nlipsy_core::serve`'s and not this module's: decode, `check`
//! outside the stack, the host's layers outermost first, `answer` at the bottom with
//! `finish` under it, the layers back innermost first, `close` over whatever came back —
//! unconditionally — and on any failure the rules of `owed` and `error_reply` instead.
//! That composer exists so that a second binding cannot re-derive it and get one of its
//! two properties wrong, and this module is the vocabulary change around it.
//!
//! Nothing a host can send reaches a `panic!`, an `unwrap` or an index here: a component
//! that trapped would take down an instance that may be answering other exchanges.

use std::sync::Arc;

use nlipsy_core::{ConformityConfig, Rejection, Served};
use nlipsy_protocol::Wire;

use crate::Component;
use crate::bindings::exports::nlipsy::engine::engine::Guest;
use crate::bindings::nlipsy::engine::types as wt;
use crate::convert::{
    config_out, findings_out, finished_out, http_options_out, message_in, message_out, millis_out,
    outcome_for_rules, owed_in, owed_out, rejected_out, rejection_out, served_out, wire_in,
};
use crate::host;

impl Guest for Component {
    fn default_config() -> wt::Config {
        config_out(&ConformityConfig::default())
    }

    fn default_rejection() -> wt::Rejection {
        rejection_out(&Rejection::default())
    }

    fn default_http_options() -> wt::HttpOptions {
        // The reference server's binding, option for option. It is assembled from
        // `nlipsy-http`'s own values rather than from `ServerConfig::default`
        // (`crates/nlipsy-server/src/server.rs:103`) because that crate is axum over
        // hyper over tokio and does not compile for `wasm32-wasip3` — but the two cannot
        // disagree, because `ServerConfig::default` is assembled from exactly these
        // three and nothing else. `cors` is absent, which is also what it does.
        http_options_out()
    }

    fn owed_by(outcome: wt::Outcome) -> wt::Owed {
        // The rule is `nlipsy_core::owed` and is asked rather than restated: it reads
        // the variant and nothing else, which is what makes rebuilding the engine's
        // `Outcome` from the contract's sound here — see `outcome_for_rules`.
        owed_out(nlipsy_core::owed(&outcome_for_rules(outcome)))
    }

    fn public_message(outcome: wt::Outcome) -> Option<String> {
        match outcome {
            // The one arm that cannot be delegated. `Outcome::public_message`
            // (`crates/nlipsy-core/src/serve.rs:84`) renders this as
            // `malformed NLIP message: {error}`, and `{error}` is the `Display` of a
            // `ParseError` — which is exactly what the contract carries in
            // `parse-error.detail`, and which no `ParseError` variant can be rebuilt
            // to print. So the sentence is written out here, over the engine's own text
            // rather than beside it.
            wt::Outcome::Malformed(error) => {
                Some(format!("malformed NLIP message: {}", error.detail))
            }
            other => outcome_for_rules(other).public_message(),
        }
    }

    fn retry_after_of(outcome: wt::Outcome) -> Option<u64> {
        // `Outcome::retry_after` over `EndpointError::retry_after`, asked rather than
        // restated: it reads the variant and the delay inside it, both of which
        // `outcome_for_rules` rebuilds exactly for the `failed` arm. A binding that
        // matched on `unavailable` itself would be a second copy of the rule — and this
        // guest's own `wasi:http/handler` is one of the callers.
        outcome_for_rules(outcome).retry_after().map(millis_out)
    }

    fn error_reply(
        request: Option<wt::Message>,
        why: String,
        owed: wt::Owed,
        cx: wt::Context,
    ) -> wt::Message {
        let request = request.map(message_in);
        message_out(&nlipsy_core::error_reply(
            &host::steps(),
            request.as_ref(),
            &why,
            owed_in(owed),
            &host::context_in(cx),
        ))
    }

    fn check(request: wt::Message, cx: wt::Context) -> Result<Vec<wt::Finding>, wt::Rejected> {
        let request = message_in(request);
        let cx = host::context_in(cx);
        match host::steps().check(&request, &cx) {
            Ok(findings) => Ok(findings_out(&findings)),
            Err(rejected) => Err(rejected_out(&rejected)),
        }
    }

    fn finish(request: wt::Message, reply: wt::Message, cx: wt::Context) -> wt::Finished {
        let request = message_in(request);
        let reply = message_in(reply);
        let cx = host::context_in(cx);
        finished_out(&host::steps().finish(&request, reply, &cx))
    }

    async fn serve(request: Vec<u8>, wire: wt::Wire, cx: wt::Context) -> wt::Served {
        served_out(&serve_bytes(request, wire_in(wire), cx).await)
    }

    async fn serve_message(request: wt::Message, cx: wt::Context) -> wt::Served {
        let conformity = Arc::new(host::composed());
        let layers = host::layers();
        let request = message_in(request);
        let mut cx = host::context_in(cx);
        served_out(&nlipsy_core::serve_message(&conformity, &layers, &request, &mut cx).await)
    }
}

/// The composed order over bytes, as the engine's own [`Served`].
///
/// `engine.serve` is this with a conversion on the way out; `wasi:http/handler` is this
/// with the ECMA-431 §6 binding around it, and it wants the value rather than the record
/// because it reads `outcome` through `Outcome::retry_after` and `StatusMap::of`, which
/// are the engine's rules and not the contract's shapes.
///
/// `wire` is a parameter and not a guess, so ECMA-432 §7.2's text fallback stays the
/// caller's decision — the same reason `protocol.decode` takes one. The decode itself is
/// the composer's, which is what makes `outcome.malformed` its answer rather than an arm
/// written here.
pub async fn serve_bytes(request: Vec<u8>, wire: Wire, cx: wt::Context) -> Served {
    let conformity = Arc::new(host::composed());
    let layers = host::layers();
    let mut cx = host::context_in(cx);
    nlipsy_core::serve(&conformity, &layers, &request, wire, &mut cx).await
}
