//! The five interfaces a deployment supplies, at their smallest.
//!
//! This is the whole of what running an NLIP end-point on this engine costs: a handler,
//! and four answers that say "nothing". Everything else — clause 5's validation, clause
//! 6's echo and ceiling and minting, the composed order, ECMA-431 §6's path and status
//! line and correlator — is in the engine component on the other side of `wac plug`, and
//! this file cannot reach any of it.
//!
//! The four "nothing" answers are the same defaults a Rust deployment gets from
//! `nlipsy-core`'s trait default methods, written out because **WIT has no optional
//! imports**: a host implements all five interfaces or the component does not
//! instantiate. The JavaScript wrapper in `javascript/` supplies exactly these.

use nlipsy_protocol::Message;

use crate::Endpoint;
use crate::bindings::exports::nlipsy::engine::host_authority::Guest as AuthorityGuest;
use crate::bindings::exports::nlipsy::engine::host_authority::{
    Context as AuthorityContext, TokenKind,
};
use crate::bindings::exports::nlipsy::engine::host_endpoint::{
    Config, Context, EndpointError, Guest as EndpointGuest, HttpOptions, Message as WitMessage,
    Rejection,
};
use crate::bindings::exports::nlipsy::engine::host_layers::{
    Context as LayerContext, EndpointError as LayerError, Guest as LayersGuest, LayerStep,
    Message as LayerMessage,
};
use crate::bindings::exports::nlipsy::engine::host_record_sink::Guest as SinkGuest;
use crate::bindings::exports::nlipsy::engine::host_transport::{
    Context as TransportContext, Exchange, Guest as TransportGuest, TransportError,
};
use crate::convert::{message_in, message_out};

/// What §6.2.1 and §6.2.2 are switched to, and how many tokens a request may carry.
///
/// Left to the engine: `engine.default-config` is "issue nothing, refuse above 32
/// tokens", which is what an end-point with no session state wants — §6.2's echo is the
/// only mandatory half of the clause and it needs no configuration at all. A deployment
/// that keeps conversations turns the switches on here and answers `issued` and `mint`
/// below.
impl EndpointGuest for Endpoint {
    fn name() -> String {
        "echo".to_owned()
    }

    fn conformity() -> Config {
        Config {
            issue_conversation_token: false,
            issue_authentication_token: false,
            max_tokens_per_message: 32,
        }
    }

    fn rejection_policy() -> Rejection {
        use crate::bindings::nlipsy::engine::types::{Disposition, Policy};
        // Reject an error, log a warning, in both directions — `Rejection::default`'s
        // answer. A message that cannot be interpreted is refused; one that is merely
        // off-spec is answered and the finding reported.
        let policy = Policy {
            on_error: Disposition::Reject,
            on_warning: Disposition::Log,
        };
        Rejection {
            inbound: policy,
            outbound: policy,
        }
    }

    fn http_binding() -> HttpOptions {
        use crate::bindings::nlipsy::engine::types::StatusMap;
        // The reference server's binding, spelled out because this component cannot call
        // the engine's `default-http-options` — it is an *export* of the component this
        // one plugs into, and `wac plug` joins imports to exports in one direction.
        // A deployment that wanted to change one of these would change it here.
        HttpOptions {
            max_request_body_size: 8 * 1024 * 1024,
            correlator_header: Some("x-nlip-correlator".to_owned()),
            cors: None,
            status: StatusMap {
                malformed: 400,
                refused: 400,
                too_large: 413,
                unsupported: 501,
                unavailable: 503,
                timed_out: 504,
                internal: 500,
                withheld: 500,
                other: 500,
            },
        }
    }

    /// Echo the request's text back.
    ///
    /// `extract_text(None)` is every `text` submessage in document order joined by
    /// spaces — the first submessage first, which §5 ¶2 makes part of the message — so a
    /// request that carries a conversation token beside its question echoes the question
    /// and not the token. What comes back is a plain `text` reply; the token the peer
    /// sent is put back on it by the engine, under §6.2 ¶2, without this end-point doing
    /// anything.
    async fn answer(request: WitMessage, cx: Context) -> Result<WitMessage, EndpointError> {
        // The context is read and not used, which is the honest shape for an end-point
        // with no state: `cx.exchange` is what a deployment that kept a session would key
        // it on, and `cx.peer`/`cx.correlator` are what it would log.
        let _ = cx;
        let request = message_in(request);
        let said = request
            .extract_text(None)
            .unwrap_or_else(|| "nothing to echo".to_owned());
        Ok(message_out(&Message::text(said)))
    }
}

/// No layers.
///
/// `count` of `0` is a deployment with none, and it is what this interface exists to let
/// a host say. The other two are unreachable and answer conservatively rather than
/// trapping: a component that trapped would take down an instance answering other
/// exchanges.
impl LayersGuest for Endpoint {
    fn count() -> u32 {
        0
    }

    async fn before(
        index: u32,
        request: LayerMessage,
        cx: LayerContext,
    ) -> Result<LayerStep, LayerError> {
        let _ = (index, cx);
        Ok(LayerStep::Pass(request))
    }

    async fn after(
        index: u32,
        reply: LayerMessage,
        cx: LayerContext,
    ) -> Result<LayerMessage, LayerError> {
        let _ = (index, cx);
        Ok(reply)
    }
}

/// An authority that owns nothing and mints nothing.
///
/// §6.2's exception says an end-point *may* decline to echo a token it created itself;
/// this one created none, so it declines nothing and every token the peer sends comes
/// back. §6.2.1 and §6.2.2 are optional and `none` is how an implementation declines
/// them — the Rust default, reached by a different route.
impl AuthorityGuest for Endpoint {
    fn is_own(token: String, cx: AuthorityContext) -> bool {
        let _ = (token, cx);
        false
    }

    fn issued(kind: TokenKind, cx: AuthorityContext) -> Option<String> {
        let _ = (kind, cx);
        None
    }

    fn mint(kind: TokenKind, cx: AuthorityContext) -> Option<String> {
        let _ = (kind, cx);
        None
    }
}

/// No transport of its own.
///
/// `client.call-through` is for a host whose runtime has no outgoing HTTP; this one runs
/// under `wasmtime serve`, which has `wasi:http/client`, so `client.call` is the path and
/// this answers `unreachable` — the conservative case of the three, because nothing was
/// sent and a caller may repeat the request freely.
impl TransportGuest for Endpoint {
    async fn send(request: Vec<u8>, cx: TransportContext) -> Result<Exchange, TransportError> {
        let _ = (request, cx);
        Err(TransportError::Unreachable(
            crate::bindings::nlipsy::engine::types::TransportFailure {
                endpoint: "echo-endpoint".to_owned(),
                detail: "this deployment supplies no transport; `client.call` goes out \
                         over wasi:http"
                    .to_owned(),
            },
        ))
    }
}

/// A sink that discards.
///
/// Recording is for a caller building a fixture. `ok` rather than an error, because a
/// deployment that records nothing has not failed at anything — and an error here would
/// become a `transport-error.malformed` on an exchange that went through perfectly well.
impl SinkGuest for Endpoint {
    async fn write(line: String) -> Result<(), String> {
        let _ = line;
        Ok(())
    }
}
