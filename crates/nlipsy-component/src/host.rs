//! The host's five interfaces, behind the engine's own traits.
//!
//! `wit/nlipsy.wit` imports five interfaces of plain functions — `host-endpoint`,
//! `host-layers`, `host-authority`, `host-transport`, `host-record-sink` — and each of
//! them becomes one of `nlipsy-core`'s seams here: [`HostEndpoint`] is an [`Endpoint`],
//! [`HostLayer`] is a [`Middleware`] per layer index, [`HostAuthority`] is a
//! [`TokenAuthority`], [`HostTransport`] is a [`Transport`], [`HostSink`] is a
//! [`RecordSink`]. Nothing here holds a handle, because there is nothing to hold: the
//! contract has no resources on either side.
//!
//! # How a host's state reaches a step
//!
//! [`TokenAuthority::is_own`] is handed a [`Context`]; the imported function takes the
//! contract's `context` **record**, so there is nothing to borrow, nothing to rebuild
//! from a handle and no `unsafe`. What a host keys its own session on is
//! [`context.exchange`](context_out) — a number this guest mints — and the same number
//! reaches every callback of one exchange, which is what makes a substituted context
//! impossible rather than merely discouraged: a layer cannot hand the engine a second
//! context and split one exchange between two.
//!
//! The number travels inside [`Context::extensions`] as a private [`Exchange`], which is
//! a `Copy` `u64` keyed by a type nothing outside this crate can name. `extensions` is
//! otherwise **unused** by this guest: it is a Rust type map and cannot cross a
//! boundary, so a deployment's own session stays on the host's side of it, reachable by
//! the exchange number and by nothing else.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use nlipsy_core::{
    Conformity, Context, Endpoint, EndpointError, Exchange as CoreExchange, Middleware, Next,
    RecordError, RecordSink, Recorded, TokenAuthority, TokenKind, Transport, TransportError,
};
use nlipsy_protocol::Message;

use crate::bindings::nlipsy::engine::types as wt;
use crate::bindings::nlipsy::engine::{
    host_authority, host_endpoint, host_layers, host_record_sink, host_transport,
};
use crate::convert::{
    config_in, endpoint_error_in, exchange_in, message_in, message_out, rejection_in,
    token_kind_out, transport_error_in,
};

// ---------------------------------------------------------------------------
// The exchange number
// ---------------------------------------------------------------------------

/// The exchange this [`Context`] belongs to.
///
/// A `Copy` `u64` in [`Context::extensions`], keyed by this private type, so nothing
/// outside this crate can put one there or read one out.
#[derive(Debug, Clone, Copy)]
struct Exchange(u64);

/// The counter the guest hands out exchange numbers from.
///
/// **Per instance, and the contract says so** (`types.context`): it identifies an
/// exchange within one component instance and is not a correlator, is never on a wire,
/// and means nothing to a second instance. Starting at `1` is what makes `0` usable as
/// the contract's "the caller names no exchange".
static NEXT_EXCHANGE: AtomicU64 = AtomicU64::new(1);

/// The engine's context for one exchange, from the contract's record.
///
/// `cx.exchange` of `0` means the caller named none and one is minted here; any other
/// value is the caller's and travels unchanged, which is what lets a corpus runner
/// drive `check` and `finish` as two steps of *one* exchange.
pub fn context_in(cx: wt::Context) -> Context {
    let exchange = match cx.exchange {
        0 => NEXT_EXCHANGE.fetch_add(1, Ordering::Relaxed),
        named => named,
    };
    let mut out = Context::new();
    out.peer = cx.peer;
    out.correlator = cx.correlator;
    out.extensions.insert(Exchange(exchange));
    out
}

/// The contract's record, from the engine's context — what every host callback is
/// handed.
///
/// An exchange of `0` is only reachable for a [`Context`] this crate did not build,
/// which no path here produces: every step is handed a [`context_in`]. It is the honest
/// answer rather than a fabricated number, and it is the same thing a host sees when it
/// passes one in.
pub fn context_out(cx: &Context) -> wt::Context {
    wt::Context {
        peer: cx.peer.clone(),
        correlator: cx.correlator.clone(),
        exchange: cx.extensions.get::<Exchange>().map_or(0, |held| held.0),
    }
}

// ---------------------------------------------------------------------------
// host-endpoint
// ---------------------------------------------------------------------------

/// The imported `host-endpoint`, behind [`Endpoint`].
///
/// It answers with a [`Message`] and not a `finished`: finishing is the guest's, `Tail`
/// puts it at the bottom of the composed stack, and there is no shape a host could use
/// to hand back a reply that skipped it.
///
/// The name is read once, when this is built, because [`Endpoint::name`] returns a
/// `&str` and a host call cannot. One call per exchange on a path that has already
/// decoded a message; what it buys is that `host-endpoint.name` is genuinely *linked*,
/// so the artefact's world declares it and a host generated from the artefact is asked
/// for it: `wasm-tools component wit` drops an import nothing calls, and a host would
/// otherwise never learn it is expected.
#[derive(Debug)]
pub struct HostEndpoint {
    name: String,
}

impl HostEndpoint {
    /// Read the deployment's name and get ready to answer one exchange.
    pub fn new() -> HostEndpoint {
        HostEndpoint {
            name: host_endpoint::name(),
        }
    }
}

impl Endpoint for HostEndpoint {
    fn name(&self) -> &str {
        &self.name
    }

    async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        match host_endpoint::answer(message_out(request), context_out(cx)).await {
            Ok(reply) => Ok(message_in(reply)),
            Err(error) => Err(endpoint_error_in(error)),
        }
    }
}

// ---------------------------------------------------------------------------
// host-layers
// ---------------------------------------------------------------------------

/// One of the host's layers, behind [`Middleware`].
///
/// The contract splits the "around" shape in two — `before` on the way in, `after` on
/// the way out — because `next` cannot cross a boundary, and this is where the two
/// halves become one call again. The order is therefore the engine's: [`Next::run`]
/// walks the stack outermost first and unwinds innermost first, and
/// `Conformity::finish` sits under all of it.
#[derive(Debug)]
pub struct HostLayer {
    index: u32,
}

impl Middleware for HostLayer {
    async fn call(
        &self,
        request: &Message,
        cx: &mut Context,
        next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        let step = host_layers::before(self.index, message_out(request), context_out(cx))
            .await
            .map_err(endpoint_error_in)?;
        let passed = match step {
            // The short circuit. The handler and every layer below are not run, and this
            // layer's own `after` is not called either: it has its reply already. What
            // the reply still owes is `Conformity::close`, which `serve` applies
            // unconditionally after the stack — the property `conformance/cases/
            // readings/51`–`/55` assert.
            wt::LayerStep::Answer(reply) => return Ok(message_in(reply)),
            wt::LayerStep::Pass(passed) => message_in(passed),
        };
        let reply = next.run(&passed, cx).await?;
        let reply = host_layers::after(self.index, message_out(&reply), context_out(cx))
            .await
            .map_err(endpoint_error_in)?;
        Ok(message_in(reply))
    }
}

/// The host's stack, outermost first, as the composer wants it.
///
/// `count()` is read once per exchange. Zero is the ordinary answer for a deployment
/// with no layers, and it is what the contract's `host-layers` exists to let a host say:
/// WIT has no optional imports and no default methods, so a deployment with no
/// layers still implements the interface and answers zero.
pub fn layers() -> Vec<std::sync::Arc<dyn nlipsy_core::DynMiddleware>> {
    (0..host_layers::count())
        .map(|index| {
            std::sync::Arc::new(HostLayer { index })
                as std::sync::Arc<dyn nlipsy_core::DynMiddleware>
        })
        .collect()
}

// ---------------------------------------------------------------------------
// host-authority
// ---------------------------------------------------------------------------

/// The imported `host-authority`, behind [`TokenAuthority`].
///
/// §6.2's exception, §6.2.1's confirmation and §6.2.1/§6.2.2's minting, each a
/// synchronous host call on the path between the handler and the wire — which is why the
/// contract makes all three synchronous and says they must be cheap. A zero-sized type:
/// there is nothing to hold.
#[derive(Debug)]
pub struct HostAuthority;

impl TokenAuthority for HostAuthority {
    fn is_own(&self, token: &str, cx: &Context) -> bool {
        host_authority::is_own(token, &context_out(cx))
    }

    fn issued(&self, kind: TokenKind, cx: &Context) -> Option<String> {
        host_authority::issued(token_kind_out(kind), &context_out(cx))
    }

    fn mint(&self, kind: TokenKind, cx: &Context) -> Option<String> {
        host_authority::mint(token_kind_out(kind), &context_out(cx))
    }
}

// ---------------------------------------------------------------------------
// host-transport
// ---------------------------------------------------------------------------

/// The imported `host-transport`, behind [`Transport`].
///
/// The alternative to `wasi:http/client` for a host whose runtime has no outgoing HTTP,
/// and for a test. Zero-sized for the same reason [`HostAuthority`] is.
#[derive(Debug)]
pub struct HostTransport;

impl Transport for HostTransport {
    async fn exchange(&self, cx: &Context, body: Vec<u8>) -> Result<CoreExchange, TransportError> {
        match host_transport::send(body, context_out(cx)).await {
            Ok(exchange) => Ok(exchange_in(exchange)),
            Err(error) => Err(transport_error_in(error)),
        }
    }
}

// ---------------------------------------------------------------------------
// host-record-sink
// ---------------------------------------------------------------------------

/// What [`Recorder`](nlipsy_core::Recorder) writes to, on its way to the host.
///
/// It is a [`RecordSink`] that **buffers**, and the reason is a mismatch the contract
/// takes on purpose. [`RecordSink::record`] is synchronous — it is called from inside
/// `Recorder`'s `Transport::exchange`, which cannot await anything of its own — while
/// `host-record-sink.write` is an `async func`, because writing is I/O and it is the
/// user's (`wit/nlipsy.wit`). So the line `Recorder` produced is held here and
/// `client.call-recording` hands it over once the exchange has returned; what decides
/// *what* is recorded is still `Recorder`, and the spelling is still
/// [`Recorded::to_json_line`], which is what lets a fixture written from JavaScript load
/// in Rust.
///
/// Nothing is swallowed: [`HostSink::take`] hands back every line, and the caller turns
/// a failed write into `transport-error.malformed`, exactly as `Recorder` turns a failed
/// `record` into one.
/// Cloneable, and the clones share one buffer: `nlipsy_core::Client` boxes its transport
/// as `'static`, so the `Recorder` inside it cannot borrow a sink the caller is still
/// holding. One handle goes into the `Recorder` and one stays with
/// `client.call-recording`, which reads back what was written.
#[derive(Debug, Default, Clone)]
pub struct HostSink {
    lines: std::sync::Arc<Mutex<Vec<String>>>,
}

impl HostSink {
    /// Every line written so far, and the buffer emptied.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .lines
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

impl RecordSink for HostSink {
    fn record(&self, exchange: &Recorded) -> Result<(), RecordError> {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(exchange.to_json_line());
        Ok(())
    }
}

/// Hand every buffered line to the host, in order.
///
/// A failure is reported and never swallowed — a recording that quietly stopped growing
/// would produce a fixture that passes for the wrong reason — and it is a
/// [`TransportError::Malformed`] because that is what `Recorder` reports a sink failure
/// as (`crates/nlipsy-core/src/record.rs`).
pub async fn flush(sink: &HostSink) -> Result<(), TransportError> {
    for line in sink.take() {
        host_record_sink::write(line)
            .await
            .map_err(|detail| TransportError::Malformed { detail })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The `Conformity` every exported call builds
// ---------------------------------------------------------------------------

/// Clause 6 with no end-point under it, for the steps that never ask for one.
///
/// `engine.check`, `engine.finish` and `engine.error-reply` are `Conformity`'s methods
/// over values; the `E` parameter is only reached through `Conformity`'s own
/// [`Endpoint`] implementation, which nothing here uses. `()` therefore costs one host
/// call less than [`HostEndpoint::new`] per call — the deployment's name is not a
/// clause-6 value.
pub fn steps() -> Conformity<(), HostAuthority> {
    Conformity::new(
        (),
        HostAuthority,
        config_in(host_endpoint::conformity()),
        rejection_in(host_endpoint::rejection_policy()),
    )
}

/// Clause 6 wrapped around the host's handler, for the composed order.
///
/// Built **per exported call** from `host-endpoint.conformity`,
/// `host-endpoint.rejection-policy` and the `host-authority` import, so that nothing in
/// this guest outlives a call and nothing needs `'static` beyond what the traits already
/// ask: `serve_answered` takes a `&Conformity` and asks for no more. It is the safe
/// shape, reached by removing the resources rather than by relaxing a published bound.
pub fn composed() -> Conformity<HostEndpoint, HostAuthority> {
    Conformity::new(
        HostEndpoint::new(),
        HostAuthority,
        config_in(host_endpoint::conformity()),
        rejection_in(host_endpoint::rejection_policy()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // These run on the *host*, through the `rlib`: wit-bindgen's generated code compiles
    // for a native target with every canonical-ABI import replaced by an `unreachable!()`
    // stub, so anything here that does not call one is testable without a wasm runtime.
    // The context projection and the sink are the whole of what this module does without
    // calling into the host.

    fn wit_context(peer: Option<&str>, exchange: u64) -> wt::Context {
        wt::Context {
            peer: peer.map(str::to_owned),
            correlator: None,
            exchange,
        }
    }

    #[test]
    fn a_caller_that_names_no_exchange_is_given_one() {
        // `0` is the contract's "no exchange named", so the guest mints, and the number
        // it minted is what every host callback of this exchange then sees.
        let cx = context_in(wit_context(Some("10.0.0.1"), 0));

        assert_eq!(cx.peer.as_deref(), Some("10.0.0.1"));
        let out = context_out(&cx);
        assert_ne!(out.exchange, 0, "the guest minted one");
        assert_eq!(out.peer.as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn a_caller_that_names_one_keeps_it_across_two_steps() {
        // The property `engine.check` then `engine.finish` depends on: a host driving
        // clause 6 as two steps of one exchange must have its authority asked under one
        // number, and `0` would have minted two.
        let first = context_out(&context_in(wit_context(None, 77)));
        let second = context_out(&context_in(wit_context(None, 77)));

        assert_eq!(first.exchange, 77);
        assert_eq!(second.exchange, 77);
    }

    #[test]
    fn two_exchanges_are_given_their_own_numbers() {
        let first = context_out(&context_in(wit_context(Some("a"), 0)));
        let second = context_out(&context_in(wit_context(Some("b"), 0)));

        assert_ne!(first.exchange, second.exchange);
    }

    #[test]
    fn a_context_this_crate_did_not_build_names_no_exchange() {
        // Unreachable — every step here is handed a `context_in` — and the answer is the
        // honest one rather than a fabricated number.
        assert_eq!(context_out(&Context::new()).exchange, 0);
    }

    #[test]
    fn the_sink_hands_back_every_line_once() {
        let sink = HostSink::default();
        let exchange = CoreExchange::new(
            b"{}".to_vec(),
            nlipsy_core::ExchangeDetail::Http {
                status: 200,
                retry_after: None,
            },
        );

        sink.record(&Recorded::of(b"{\"a\":1}", &exchange))
            .expect("the buffer accepts one");
        sink.record(&Recorded::of(b"{\"a\":2}", &exchange))
            .expect("and a second");

        let lines = sink.take();
        assert_eq!(lines.len(), 2, "one line per exchange, in order");
        assert!(lines[0].contains("\\\"a\\\":1"));
        assert!(
            sink.take().is_empty(),
            "taken once: a line handed to the host is not written twice"
        );
    }
}
