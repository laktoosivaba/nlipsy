//! The corpus, run through the built wasm **component** in wasmtime.
//!
//! `crates/nlipsy-component` compiles the engine against `wit/nlipsy.wit` and
//! `cargo build --target wasm32-wasip3` turns it into a component. This module loads
//! that artefact, implements the contract's five `host-*` interfaces with the same
//! scripted values [`crate::exchange`] drives the Rust API with, and runs **every** case
//! through it — the first proof of the artefact that needs no JavaScript at all.
//!
//! It is not a second runner. The case types, the walk, the report and every expectation
//! are [`crate`]'s: what differs is only where the answers come from, which is what
//! [`Implementation`] is for. A component that passed a corpus of its own would be
//! evidence about that corpus.
//!
//! # No resources on either side
//!
//! The contract has none (`wit/README.md`, *The design*). The host's five interfaces are
//! free functions, so there is no resource table and no handle: [`HostState`] *is* the
//! deployment — one end-point, one layer stack, one authority, one transport, one sink —
//! and a case configures it before it calls. The context is a record by value carrying
//! an `exchange` number, so a case that drives `engine.check` and `engine.finish` as two
//! steps of one exchange passes the same number to both and the scripted authority sees
//! one exchange rather than two.
//!
//! The `short_circuit` cases take the route a Rust deployment takes: layers are the
//! guest's, so the case is **a host layer whose `before` answers** —
//! `host-layers.count()` is `1`, `before(0, …)` returns
//! `layer-step.answer(endpoint.reply)` — and there is no way for a host to present a
//! finished reply at all, because `engine.finish` is the sole producer of one.
//!
//! # What this costs and why it is not in `make check`
//!
//! A WebAssembly runtime, the `wasm32-wasip3` target on a nightly toolchain — the
//! target is stable at Rust 1.100 (2026-11-12) and not before — and a built artefact.
//! `make check-component` has all three; `make check` must stay runnable with cargo and
//! nothing else, which is the same argument that keeps `check-wasm` and `check-wit` out
//! of it.

use std::path::{Path, PathBuf};

use nlipsy_protocol::{Content, Format, Message, ParseErrorKind, Submessage};
use serde_json::Value;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView, default_hooks};

use crate::exchange::{
    AuthorityScript, ExchangeCase, ObservedRefusal, ScriptedAuthority, compare_findings,
    compare_refusal, compare_reply, compare_withheld, malformed_case, rejection_of,
};
use crate::{
    Case, Differences, ErrorDescription, ExpectedViolation, Implementation, Observed,
    ObservedToken, Outcome, canonical_content, canonical_model, compare_description,
    compare_observed, observed_token,
};

/// The contract, as a wasmtime host reads it.
///
/// In a module of its own so that the workspace's `missing_docs` and
/// `missing_debug_implementations` lints — which `make check` turns into errors — apply
/// to what is written here and not to four thousand generated lines.
/// The contract, as a wasmtime host reads it.
///
/// In a module of its own so that the workspace's `missing_docs` and
/// `missing_debug_implementations` lints — which `make check` turns into errors — apply
/// to what is written here and not to four thousand generated lines.
///
/// **A narrower world than the guest's**, and deliberately. `wit/nlipsy.wit`'s `nlipsy`
/// world also exports `wasi:http/handler@0.3.0` and imports `wasi:http/client@0.3.0` and
/// `wasi:random/random@0.3.0`, and every one of those is the *runtime's* to implement:
/// `wasmtime_wasi::p3::add_to_linker` and `wasmtime_wasi_http::p3::add_to_linker`
/// already do, and `wasmtime_wasi_http::p3::bindings::Service::new` finds the `handler`
/// export on the same instance without any binding of ours. Generating them here would
/// ask `HostState` to implement thirty `wasi:http/types` resource methods for nothing.
/// So the world below names the five interfaces this host actually supplies and the
/// three interfaces it actually calls.
#[allow(missing_docs, missing_debug_implementations)]
mod bindings {
    wasmtime::component::bindgen!({
        // The `path` is still the contract, because the world below refers to its
        // interfaces by name: `inline` says which of them this host binds, `path` is
        // where they are read from, so the two cannot drift.
        inline: "
            package nlipsy:conformance-host@0.1.0;

            /// What the corpus host supplies to the engine, and what it calls on it.
            world nlipsy-host {
              import nlipsy:engine/host-endpoint@0.1.0;
              import nlipsy:engine/host-layers@0.1.0;
              import nlipsy:engine/host-authority@0.1.0;
              import nlipsy:engine/host-transport@0.1.0;
              import nlipsy:engine/host-record-sink@0.1.0;
              export nlipsy:engine/protocol@0.1.0;
              export nlipsy:engine/engine@0.1.0;
              export nlipsy:engine/client@0.1.0;
            }
        ",
        path: "../../wit",
        world: "nlipsy-host",
        // Every export is called through `*_async`, and it is the *imports* that make
        // that necessary rather than a choice made here: `host-endpoint.answer`,
        // `host-layers.before`/`after`, `host-transport.send` and
        // `host-record-sink.write` are `async func` in the contract, so the instance is
        // an asynchronous one and wasmtime refuses a synchronous call into a store that
        // holds one ("store configuration requires that `*_async` functions are used
        // instead"). `crate::component::block_on` is the whole executor that needs.
        exports: { default: async },
    });
}

use bindings::NlipsyHost;
use bindings::exports::nlipsy::engine::client::Guest as ClientGuest;
use bindings::exports::nlipsy::engine::engine::Guest as EngineGuest;
use bindings::exports::nlipsy::engine::protocol::Guest as ProtocolGuest;
use bindings::nlipsy;
use bindings::nlipsy::engine::types as wt;

/// How one exchange ended, as the *contract* spells it — re-exported under a name of its
/// own so that a test can match the tag without reaching into the generated module, and
/// without colliding with the corpus's own [`crate::Outcome`], which is a case's verdict
/// and a different thing entirely.
pub use bindings::nlipsy::engine::types::Outcome as ServedOutcome;

/// The contract's own vocabulary, for a test that has to build or read one.
///
/// Re-exported here rather than reached through `bindings`, which is private so that the
/// workspace's `missing_docs` lint does not apply to four thousand generated lines.
pub use bindings::nlipsy::engine::types::{
    Config as WitConfig, Content as WitContent, Context as WitContext, Cors, Disposition,
    EndpointError, Exchange, ExchangeDetail, Finding, Finished, Format as WitFormat, HttpDetail,
    HttpOptions, LayerStep, Message as WitMessage, Number, Owed, Policy, Rejected, Rejection,
    Reply, Served, StatusMap, Submessage as WitSubmessage, TokenKind, TransportError, Unavailable,
    Violation, Wire,
};

/// Where the artefact is when nobody says otherwise.
///
/// Relative to this crate's manifest, so it is the path
/// `cargo +nightly build -p nlipsy-component --profile component --target wasm32-wasip3`
/// writes, whatever directory the test was started from.
const DEFAULT_ARTEFACT: &str = "../../target/wasm32-wasip3/component/nlipsy_component.wasm";

/// The environment variable that overrides it.
pub const ARTEFACT_ENV: &str = "NLIPSY_COMPONENT";

/// Where the component is.
pub fn artefact_path() -> PathBuf {
    match std::env::var_os(ARTEFACT_ENV) {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_ARTEFACT),
    }
}

// ---------------------------------------------------------------------------
// The deployment a case scripts
// ---------------------------------------------------------------------------

/// The `host-authority` a case or a test scripts.
///
/// Two, because the corpus and the concurrency assertion ask different things of one.
/// A case scripts its three answers as data and says nothing about the context, which is
/// right: clause 6 consults neither the peer nor the correlator. The assertion needs the
/// opposite — an authority whose answer *depends* on the context — because "each
/// exchange saw its own" is precisely what it is checking.
#[derive(Debug)]
pub enum CaseAuthority {
    /// A case's `own` / `issued` / `mint` block, answered by the same
    /// [`ScriptedAuthority`] the Rust run drives.
    Scripted(Box<ScriptedAuthority>),
    /// Mints `tok-<peer>` from the context it is handed, and recognises nothing as its
    /// own. What a token minted for exchange A looks like if exchange B's context ever
    /// reached it.
    FromPeer,
}

impl Default for CaseAuthority {
    fn default() -> CaseAuthority {
        CaseAuthority::Scripted(Box::new(
            ScriptedAuthority::new(&AuthorityScript::default()),
        ))
    }
}

/// What the scripted `answer` hands back.
#[derive(Debug)]
pub enum Answer {
    /// This message, as the end-point's own reply. The guest finishes it.
    Plain(Box<wt::Message>),
    /// Refuse to answer.
    Failing(Box<wt::EndpointError>),
    /// Answer with what the instance's **`engine.check` export** says about the request,
    /// called **from inside `answer`** — a synchronous export re-entered while the
    /// asynchronous one that reached this import is still suspended on it. The reply's
    /// text is `checked <n>`, so that the re-entry having completed is observable.
    ViaCheck,
}

/// One scripted answer, and what it records about the order it ran in.
///
/// Chosen by the request's own text rather than by arrival order, so that a test with
/// two exchanges in flight is asserting what the end-point did with *each request*
/// rather than which of the two the scheduler happened to poll first.
#[derive(Debug)]
pub struct Turn {
    /// The request text this turn answers. Empty matches any request.
    pub when: String,
    /// What to record in [`HostState::entered`] and [`HostState::resolved`]. Empty
    /// records nothing.
    pub label: String,
    /// Do not answer until this label has been entered. Empty answers straight away.
    pub after: String,
    /// What to answer with.
    pub answer: Answer,
}

/// The end-point a case or a test scripts.
///
/// The corpus never reaches `answer` on an ordinary exchange case — that is `check` then
/// `finish` — so an empty script is the default; `tests/component_async.rs` is where one
/// is filled in, and a `short_circuit` case answers above it in a layer.
#[derive(Debug, Default)]
pub struct CaseEndpoint {
    /// What `answer` hands back, one turn at a time.
    pub turns: Vec<Turn>,
}

impl CaseEndpoint {
    /// A script of one turn that answers anything.
    pub fn answering(answer: Answer) -> CaseEndpoint {
        CaseEndpoint {
            turns: vec![Turn {
                when: String::new(),
                label: String::new(),
                after: String::new(),
                answer,
            }],
        }
    }

    /// Take the turn that answers `text`, if there is one left.
    fn take_turn(&mut self, text: &str) -> Option<Turn> {
        let index = self
            .turns
            .iter()
            .position(|turn| turn.when.is_empty() || turn.when == text)?;
        Some(self.turns.remove(index))
    }
}

/// What one scripted layer's `before` half answers.
#[derive(Debug, Default)]
pub enum Before {
    /// Hand the request down unchanged.
    #[default]
    Pass,
    /// Answer here, without the handler and without the layers below — the shape
    /// `endpoint.short_circuit` is.
    Answer(Box<wt::Message>),
    /// Refuse the exchange, as a Rust layer that returns without calling `Next::run`
    /// does.
    Failing(Box<wt::EndpointError>),
}

/// One of the deployment's layers.
///
/// `label` is what it writes into [`HostState::entered`] on the way in and
/// [`HostState::resolved`] on the way out, which is how a test asserts the composed
/// order is the engine's: outermost first in, innermost first out.
#[derive(Debug, Default)]
pub struct CaseLayer {
    /// The name this layer records.
    pub label: String,
    /// What its `before` half answers.
    pub before: Before,
    /// Appended to the reply's text content on the way out, when it is set — so that
    /// the unwinding order is visible in the message itself and not only in a log.
    pub after_suffix: Option<String>,
}

/// A transport answering from a queue, for `client.call-through`.
#[derive(Debug, Default)]
pub struct CaseTransport {
    /// What it answers with, in order.
    pub queue: Vec<wt::Exchange>,
    /// What went out through it.
    pub sent: Vec<Vec<u8>>,
}

/// A sink that keeps what it was handed, in `Recorded`'s JSONL spelling.
#[derive(Debug, Default)]
pub struct CaseSink {
    /// One line per recorded exchange.
    pub lines: Vec<String>,
}

/// Everything the component can reach: **one deployment**, scripted.
///
/// There is no resource table for the contract's own types, because there are no
/// resources: the five `host-*` interfaces are free functions, so the instance has one
/// end-point, one layer stack, one authority, one transport and one sink, and a case
/// replaces them before it calls.
pub struct HostState {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    /// What `host-endpoint.name` answers.
    pub name: String,
    /// What `host-endpoint.conformity` answers.
    pub config: wt::Config,
    /// What `host-endpoint.rejection-policy` answers.
    pub rejection: wt::Rejection,
    /// What `host-endpoint.http-binding` answers.
    pub http_options: wt::HttpOptions,
    /// What `host-endpoint.answer` answers.
    pub endpoint: CaseEndpoint,
    /// The deployment's layers, outermost first.
    pub layers: Vec<CaseLayer>,
    /// What the three `host-authority` functions answer.
    pub authority: CaseAuthority,
    /// What `host-transport.send` answers.
    pub transport: CaseTransport,
    /// What `host-record-sink.write` keeps.
    pub sink: CaseSink,
    /// The instance itself, so that a host callback can re-enter it. Filled in after
    /// instantiation, which is the only order there is: the store has to exist before
    /// the component is instantiated into it.
    instance: Option<wasmtime::component::Instance>,
    /// What was entered, in order — layers on the way in and end-points.
    pub entered: Vec<String>,
    /// What answered, in order — layers on the way out and end-points.
    pub resolved: Vec<String>,
}

impl HostState {
    fn new() -> HostState {
        HostState {
            // Inherited stdio, because a panic in the guest — which `panic = "abort"`
            // makes a trap — writes its message to stderr on the way out, and a harness
            // that swallowed it would report "the call trapped" and nothing else.
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            http: WasiHttpCtx::default(),
            table: ResourceTable::new(),
            name: "corpus".to_owned(),
            config: default_config_of(false),
            rejection: default_rejection_of(),
            http_options: default_http_options_of(),
            endpoint: CaseEndpoint::default(),
            layers: Vec::new(),
            authority: CaseAuthority::default(),
            transport: CaseTransport::default(),
            sink: CaseSink::default(),
            instance: None,
            entered: Vec::new(),
            resolved: Vec::new(),
        }
    }
}

impl std::fmt::Debug for HostState {
    /// Hand-written because `WasiCtx` has no `Debug`, and what a failing case wants to
    /// see is the order things ran in.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostState")
            .field("layers", &self.layers.len())
            .field("turns", &self.endpoint.turns.len())
            .field("entered", &self.entered)
            .field("resolved", &self.resolved)
            .finish_non_exhaustive()
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for HostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: default_hooks(),
        }
    }
}

impl nlipsy::engine::types::Host for HostState {}

// ---------------------------------------------------------------------------
// The five host interfaces
// ---------------------------------------------------------------------------

impl nlipsy::engine::host_endpoint::Host for HostState {
    fn name(&mut self) -> String {
        self.name.clone()
    }

    fn conformity(&mut self) -> wt::Config {
        self.config
    }

    fn rejection_policy(&mut self) -> wt::Rejection {
        self.rejection
    }

    fn http_binding(&mut self) -> wt::HttpOptions {
        self.http_options.clone()
    }
}

impl nlipsy::engine::host_layers::Host for HostState {
    fn count(&mut self) -> u32 {
        u32::try_from(self.layers.len()).unwrap_or(u32::MAX)
    }
}

impl nlipsy::engine::host_authority::Host for HostState {
    fn is_own(&mut self, token: String, _cx: wt::Context) -> bool {
        // The *same* scripted authority the Rust runner drives, asked the same question.
        // Its answers ignore the context — a case scripts `own`, `issued` and `mint` as
        // data — so the core `Context` it is handed is the empty one
        // `crates/conformance/src/exchange.rs` passes for the same reason.
        match &self.authority {
            CaseAuthority::Scripted(authority) => nlipsy_core::TokenAuthority::is_own(
                authority.as_ref(),
                &token,
                &nlipsy_core::Context::new(),
            ),
            CaseAuthority::FromPeer => false,
        }
    }

    fn issued(&mut self, kind: wt::TokenKind, _cx: wt::Context) -> Option<String> {
        match &self.authority {
            CaseAuthority::Scripted(authority) => nlipsy_core::TokenAuthority::issued(
                authority.as_ref(),
                token_kind(kind),
                &nlipsy_core::Context::new(),
            ),
            CaseAuthority::FromPeer => None,
        }
    }

    fn mint(&mut self, kind: wt::TokenKind, cx: wt::Context) -> Option<String> {
        match &self.authority {
            CaseAuthority::Scripted(authority) => nlipsy_core::TokenAuthority::mint(
                authority.as_ref(),
                token_kind(kind),
                &nlipsy_core::Context::new(),
            ),
            // Minted *from the context this call was given*, which is the whole of the
            // concurrency assertion: two exchanges in flight must not be able to hand
            // each other's context to the authority.
            CaseAuthority::FromPeer => cx.peer.map(|peer| format!("tok-{peer}")),
        }
    }
}

fn token_kind(kind: wt::TokenKind) -> nlipsy_core::TokenKind {
    match kind {
        wt::TokenKind::Conversation => nlipsy_core::TokenKind::Conversation,
        wt::TokenKind::Authentication => nlipsy_core::TokenKind::Authentication,
    }
}

// ---------------------------------------------------------------------------
// The four asynchronous imports
// ---------------------------------------------------------------------------

impl nlipsy::engine::host_endpoint::HostWithStore<HostState> for HasSelf<HostState> {
    async fn answer(
        accessor: &wasmtime::component::Accessor<HostState, Self>,
        request: wt::Message,
        cx: wt::Context,
    ) -> Result<wt::Message, wt::EndpointError> {
        // Which turn this request is, chosen by the request's own text.
        let asked = match &request.content {
            wt::Content::Text(text) => text.clone(),
            _ => String::new(),
        };
        let turn = accessor.with(|mut access| access.get().endpoint.take_turn(&asked));
        let (label, after, answer) = match turn {
            Some(turn) => (turn.label, turn.after, Some(turn.answer)),
            None => (String::new(), String::new(), None),
        };

        if !label.is_empty() {
            accessor.with(|mut access| access.get().entered.push(label.clone()));
        }

        // The concurrency assertion's half: this end-point does not answer until the
        // other one has been entered, so a run in which the two exchanges were serialised
        // never finishes rather than passing by accident.
        if !after.is_empty() {
            while !accessor.with(|mut access| access.get().entered.contains(&after)) {
                yield_once().await;
            }
        }

        let answered = match answer {
            Some(Answer::Plain(message)) => Ok(*message),
            Some(Answer::Failing(error)) => Err(*error),
            Some(Answer::ViaCheck) => check_from_inside(accessor, request, cx).await,
            None => Err(wt::EndpointError::Unsupported(
                "this end-point was scripted with no answer".to_owned(),
            )),
        };

        if !label.is_empty() {
            accessor.with(|mut access| access.get().resolved.push(label.clone()));
        }
        answered
    }
}

/// `engine.check`, called from inside the host's `answer`.
///
/// **The re-entry.** A *synchronous* export is called while the asynchronous export
/// that reached this import is still suspended on it, which is a property the contract
/// rests on: a host callback that asks the engine a question is an ordinary thing to
/// write.
///
/// It works with one condition, and the condition is the whole reason for the yield
/// below: the canonical ABI clears the instance's `may_enter` flag for the duration of a
/// lifted call and an asynchronous export only gets it back once it has returned to its
/// event loop. wasmtime calls the host's import function while the guest is still on the
/// stack, so a re-entry on the *first* poll of this future is `wasm trap: cannot enter
/// component instance`. One yield is enough, and every real handler has one — it is what
/// `await` on anything at all does.
async fn check_from_inside(
    accessor: &wasmtime::component::Accessor<HostState, HasSelf<HostState>>,
    request: wt::Message,
    cx: wt::Context,
) -> Result<wt::Message, wt::EndpointError> {
    yield_once().await;

    let prepared = accessor.with(|mut access| {
        let instance = access
            .get()
            .instance
            .ok_or("the host has no instance handle to re-enter")?;
        let engine =
            NlipsyHost::new(&mut access, &instance).map_err(|error| format!("{error:?}"))?;
        let raw = *engine.nlipsy_engine_engine().func_check().func();
        // Re-typed with *owned* parameters, because `call_concurrent` needs them
        // `'static`; `Message` and `&Message` lower identically, and `Func::typed` checks
        // that against the component's own declaration.
        raw.typed::<(wt::Message, wt::Context), (Result<Vec<wt::Finding>, wt::Rejected>,)>(&access)
            .map_err(|error| format!("{error:?}"))
    });

    let check = match prepared {
        Ok(check) => check,
        Err(why) => {
            return Err(wt::EndpointError::Internal(format!(
                "the `check` export could not be reached from inside `answer`: {why}"
            )));
        }
    };
    match check.call_concurrent(accessor, (request, cx)).await {
        Ok((Ok(findings),)) => Ok(text_message(&format!("checked {}", findings.len()))),
        Ok((Err(rejected),)) => Err(wt::EndpointError::Refused(rejected)),
        Err(error) => Err(wt::EndpointError::Internal(format!(
            "the `check` export could not be called from inside `answer`: {error:?}"
        ))),
    }
}

impl nlipsy::engine::host_layers::HostWithStore<HostState> for HasSelf<HostState> {
    async fn before(
        accessor: &wasmtime::component::Accessor<HostState, Self>,
        index: u32,
        request: wt::Message,
        _cx: wt::Context,
    ) -> Result<wt::LayerStep, wt::EndpointError> {
        let scripted = accessor.with(|mut access| {
            let host = access.get();
            let layer = host.layers.get(index as usize);
            let label = layer.map(|layer| layer.label.clone()).unwrap_or_default();
            if !label.is_empty() {
                host.entered.push(format!("before:{label}"));
            }
            match layer.map(|layer| &layer.before) {
                Some(Before::Pass) | None => Ok(None),
                Some(Before::Answer(reply)) => Ok(Some((**reply).clone())),
                Some(Before::Failing(error)) => Err((**error).clone()),
            }
        });
        match scripted {
            Ok(None) => Ok(wt::LayerStep::Pass(request)),
            Ok(Some(reply)) => Ok(wt::LayerStep::Answer(reply)),
            Err(error) => Err(error),
        }
    }

    async fn after(
        accessor: &wasmtime::component::Accessor<HostState, Self>,
        index: u32,
        reply: wt::Message,
        _cx: wt::Context,
    ) -> Result<wt::Message, wt::EndpointError> {
        accessor.with(|mut access| {
            let host = access.get();
            let Some(layer) = host.layers.get(index as usize) else {
                return Ok(reply);
            };
            let label = layer.label.clone();
            let suffix = layer.after_suffix.clone();
            if !label.is_empty() {
                host.resolved.push(format!("after:{label}"));
            }
            let mut reply = reply;
            if let (Some(suffix), wt::Content::Text(text)) = (suffix, &reply.content) {
                reply.content = wt::Content::Text(format!("{text}{suffix}"));
            }
            Ok(reply)
        })
    }
}

// The two interfaces whose every function is `async func` still declare a synchronous
// `Host` trait with nothing in it, and wasmtime's `add_to_linker` asks for it: the
// asynchronous half is `HostWithStore`, below.
impl nlipsy::engine::host_transport::Host for HostState {}

impl nlipsy::engine::host_record_sink::Host for HostState {}

impl nlipsy::engine::host_transport::HostWithStore<HostState> for HasSelf<HostState> {
    async fn send(
        accessor: &wasmtime::component::Accessor<HostState, Self>,
        request: Vec<u8>,
        _cx: wt::Context,
    ) -> Result<wt::Exchange, wt::TransportError> {
        accessor.with(|mut access| {
            let transport = &mut access.get().transport;
            if transport.queue.is_empty() {
                return Err(wt::TransportError::Malformed(
                    "this transport was scripted with no more exchanges".to_owned(),
                ));
            }
            transport.sent.push(request);
            Ok(transport.queue.remove(0))
        })
    }
}

impl nlipsy::engine::host_record_sink::HostWithStore<HostState> for HasSelf<HostState> {
    async fn write(
        accessor: &wasmtime::component::Accessor<HostState, Self>,
        line: String,
    ) -> Result<(), String> {
        accessor.with(|mut access| {
            access.get().sink.lines.push(line);
            Ok(())
        })
    }
}

/// A `text` message, for the two places this host has to build one.
fn text_message(text: &str) -> wt::Message {
    message_out(&Message::text(text))
}

/// Hand the scheduler back the task once.
///
/// A component-model-async task that waits on another one needs somewhere to yield, and
/// this is the whole of it: wake immediately and report `Pending`, so whatever executor
/// is driving `Store::run_concurrent` polls the other task before coming back.
async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(move |cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

// ---------------------------------------------------------------------------
// The contract's vocabulary and the corpus's
// ---------------------------------------------------------------------------
//
// The mirror image of `crates/nlipsy-component/src/convert.rs`, and deliberately a
// second copy: a host speaks the contract, and one that reached into the guest's crate
// for its conversions would be testing the guest against itself. What is *not* copied is
// any expectation — those are `crate`'s, and this module ends by handing them values.

fn format_out(format: Format) -> wt::Format {
    match format {
        Format::Text => wt::Format::Text,
        Format::Token => wt::Format::Token,
        Format::Structured => wt::Format::Structured,
        Format::Binary => wt::Format::Binary,
        Format::Location => wt::Format::Location,
        Format::Generic => wt::Format::Generic,
    }
}

fn format_in(format: wt::Format) -> Format {
    match format {
        wt::Format::Text => Format::Text,
        wt::Format::Token => Format::Token,
        wt::Format::Structured => Format::Structured,
        wt::Format::Binary => Format::Binary,
        wt::Format::Location => Format::Location,
        wt::Format::Generic => Format::Generic,
    }
}

fn content_out(content: &Content) -> wt::Content {
    match content {
        Content::Null => wt::Content::Null,
        Content::Bool(value) => wt::Content::Bool(*value),
        Content::Number(number) => {
            if let Some(value) = number.as_i64() {
                wt::Content::Number(wt::Number::Int(value))
            } else if let Some(value) = number.as_u64() {
                wt::Content::Number(wt::Number::Uint(value))
            } else {
                wt::Content::Number(wt::Number::Float(number.as_f64().unwrap_or_default()))
            }
        }
        Content::String(text) => wt::Content::Text(text.clone()),
        Content::Bytes(bytes) => wt::Content::Bytes(bytes.clone()),
        // The host's half of the `structured` invariant: the canonical JSON text of the
        // value, built with `serde_json`, exactly as a JavaScript wrapper builds it with
        // `JSON.stringify` (`wit/nlipsy.wit`, the note on `content`).
        Content::Array(_) | Content::Object(_) => {
            wt::Content::Structured(content.to_json_value().to_string())
        }
    }
}

fn content_in(content: wt::Content) -> Content {
    match content {
        wt::Content::Null => Content::Null,
        wt::Content::Bool(value) => Content::Bool(value),
        wt::Content::Number(wt::Number::Int(value)) => Content::Number(value.into()),
        wt::Content::Number(wt::Number::Uint(value)) => Content::Number(value.into()),
        wt::Content::Number(wt::Number::Float(value)) => {
            match serde_json::Number::from_f64(value) {
                Some(number) => Content::Number(number),
                None => Content::Null,
            }
        }
        wt::Content::Text(text) => Content::String(text),
        wt::Content::Bytes(bytes) => Content::Bytes(bytes),
        wt::Content::Structured(json) => match serde_json::from_str::<Value>(&json) {
            Ok(value) => Content::from(value),
            Err(_) => Content::String(json),
        },
    }
}

fn submessage_out(submessage: &Submessage) -> wt::Submessage {
    wt::Submessage {
        label: submessage.label.clone(),
        format: format_out(submessage.format),
        subformat: submessage.subformat.clone(),
        content: content_out(&submessage.content),
    }
}

/// One contract submessage as the engine's own type, for an expectation to compare
/// against — the inverse of `submessage_out` over everything the record carries,
/// `label` included.
pub fn submessage_in(submessage: &wt::Submessage) -> Submessage {
    let mut out = Submessage::new(
        format_in(submessage.format),
        submessage.subformat.clone(),
        content_in(submessage.content.clone()),
    );
    out.label = submessage.label.clone();
    out
}

/// A core message as the contract carries one.
pub fn message_out(message: &Message) -> wt::Message {
    wt::Message {
        message_type: message.message_type.clone(),
        control: message.control,
        label: message.label.clone(),
        format: format_out(message.format),
        subformat: message.subformat.clone(),
        content: content_out(&message.content),
        submessages: message.submessages.iter().map(submessage_out).collect(),
    }
}

/// A contract message as the corpus's canonical model is projected from.
///
/// Lossless, which is what lets the expectations be compared by the same code the Rust
/// run uses: `format` is a closed enum, `content` is the seven cases with the two
/// recursive ones behind a JSON string the host itself wrote, and nothing else in the
/// record is derived.
pub fn message_in(message: &wt::Message) -> Message {
    let mut out = Message::new(
        format_in(message.format),
        message.subformat.clone(),
        content_in(message.content.clone()),
    );
    out.message_type = message.message_type.clone();
    out.control = message.control;
    out.label = message.label.clone();
    out.submessages = message.submessages.iter().map(submessage_in).collect();
    out
}

/// A contract `parse-error-kind` in the corpus's spelling, as `expect.error.kind`
/// writes it.
///
/// The nine strings are not written out here. `ParseErrorKind::as_str` is where the
/// kebab-case spelling lives, because it is not the runner's opinion but what the
/// implementation claims to have distinguished — so this is the one thing that is
/// genuinely this module's: the contract's enum onto the crate's, case by case.
///
/// Exhaustive on both sides and with no `_` arm, which is the point of the shape. A
/// variant added to `types.parse-error-kind` in `wit/nlipsy.wit` stops this compiling
/// until somebody decides what it is, rather than landing on a string nothing checks.
fn parse_error_kind(kind: wt::ParseErrorKind) -> &'static str {
    let kind = match kind {
        wt::ParseErrorKind::InvalidJson => ParseErrorKind::InvalidJson,
        wt::ParseErrorKind::Cbor => ParseErrorKind::Cbor,
        wt::ParseErrorKind::NotObject => ParseErrorKind::NotObject,
        wt::ParseErrorKind::MissingField => ParseErrorKind::MissingField,
        wt::ParseErrorKind::WrongType => ParseErrorKind::WrongType,
        wt::ParseErrorKind::DuplicateField => ParseErrorKind::DuplicateField,
        wt::ParseErrorKind::UnknownFormat => ParseErrorKind::UnknownFormat,
        wt::ParseErrorKind::Base64 => ParseErrorKind::Base64,
        wt::ParseErrorKind::Unmapped => ParseErrorKind::Unmapped,
    };
    kind.as_str()
}

/// One contract submessage as the corpus compares a token.
fn observed_token_of(token: &wt::Submessage) -> ObservedToken {
    observed_token(&submessage_in(token))
}

fn described(error: &wt::ParseError) -> ErrorDescription {
    ErrorDescription {
        kind: parse_error_kind(error.kind),
        path: error.path.clone(),
        field: error.field.clone(),
    }
}

fn severity(severity: wt::Severity) -> &'static str {
    match severity {
        wt::Severity::Error => "error",
        wt::Severity::Warning => "warning",
    }
}

fn violation(violation: &wt::Violation) -> ExpectedViolation {
    ExpectedViolation {
        rule: violation.rule.clone(),
        severity: severity(violation.severity).to_owned(),
        path: violation.path.clone(),
    }
}

fn refusal(rejected: &wt::Rejected) -> ObservedRefusal {
    match rejected {
        wt::Rejected::Violations(violations) => ObservedRefusal {
            kind: "violations",
            rules: violations.iter().map(|one| one.rule.clone()).collect(),
            count: None,
            limit: None,
        },
        wt::Rejected::TooManyTokens(ceiling) => ObservedRefusal {
            kind: "too-many-tokens",
            rules: Vec::new(),
            count: Some(ceiling.count as usize),
            limit: Some(ceiling.limit as usize),
        },
    }
}

fn finding(finding: &wt::Finding) -> Value {
    let disposition = match finding.disposition {
        wt::Disposition::Reject => "reject",
        wt::Disposition::Log => "log",
        wt::Disposition::Ignore => "ignore",
    };
    serde_json::json!({
        "rule": finding.violation.rule,
        "severity": severity(finding.violation.severity),
        "path": finding.violation.path,
        "disposition": disposition,
    })
}

/// One finding, tagged with the direction that reported it, in the shape
/// [`compare_findings`] compares.
fn tagged(direction: &str, findings: &[wt::Finding]) -> Vec<Value> {
    findings
        .iter()
        .map(|one| {
            let mut value = finding(one);
            if let Some(object) = value.as_object_mut() {
                object.insert("direction".to_owned(), Value::from(direction));
            }
            value
        })
        .collect()
}

fn policy_out(policy: &nlipsy_core::Policy) -> wt::Policy {
    wt::Policy {
        on_error: disposition_out(policy.on_error),
        on_warning: disposition_out(policy.on_warning),
    }
}

fn disposition_out(disposition: nlipsy_core::Disposition) -> wt::Disposition {
    match disposition {
        nlipsy_core::Disposition::Reject => wt::Disposition::Reject,
        nlipsy_core::Disposition::Log => wt::Disposition::Log,
        nlipsy_core::Disposition::Ignore => wt::Disposition::Ignore,
    }
}

// ---------------------------------------------------------------------------
// The values a case does not state
// ---------------------------------------------------------------------------

/// The contract's `default-config`, with §6.2.1's issuance switched on or off.
///
/// The defaults are the engine's own, read through the same crate the component was
/// built from rather than re-typed — the reason the contract exports them at all, applied
/// to the tests as well as to the corpus.
pub fn default_config_of(issue_conversation_token: bool) -> wt::Config {
    let theirs = nlipsy_core::ConformityConfig::default();
    wt::Config {
        issue_conversation_token,
        issue_authentication_token: theirs.issue_authentication_token,
        max_tokens_per_message: u32::try_from(theirs.max_tokens_per_message).unwrap_or(u32::MAX),
    }
}

/// The contract's `default-rejection`: reject an error, log a warning, both ways.
pub fn default_rejection_of() -> wt::Rejection {
    let theirs = nlipsy_core::Rejection::default();
    wt::Rejection {
        inbound: policy_out(&theirs.inbound),
        outbound: policy_out(&theirs.outbound),
    }
}

/// The contract's `default-http-options`, read out of `nlipsy-http` rather than re-typed.
///
/// The one value with no Rust seed is `cors`, and the answer here is the same as the
/// axum binding's: none (`wit/README.md` §1).
pub fn default_http_options_of() -> wt::HttpOptions {
    let statuses = nlipsy_http::StatusMap::default();
    wt::HttpOptions {
        max_request_body_size: u32::try_from(nlipsy_http::DEFAULT_MAX_REQUEST_BODY_SIZE)
            .unwrap_or(u32::MAX),
        correlator_header: Some(nlipsy_http::CORRELATOR_HEADER.to_owned()),
        cors: None,
        status: wt::StatusMap {
            malformed: statuses.malformed.as_u16(),
            refused: statuses.refused.as_u16(),
            too_large: statuses.too_large.as_u16(),
            unsupported: statuses.unsupported.as_u16(),
            unavailable: statuses.unavailable.as_u16(),
            timed_out: statuses.timed_out.as_u16(),
            internal: statuses.internal.as_u16(),
            withheld: statuses.withheld.as_u16(),
            other: statuses.other.as_u16(),
        },
    }
}

/// A `context` naming a peer and an exchange.
///
/// `exchange` is the contract's own number and `0` means "the caller names none", which
/// is what a runner driving one call passes. A runner driving `check` **and** `finish`
/// as two steps of one exchange passes the same non-zero number to both, so the
/// authority is asked under one exchange rather than two — see `types.context`.
pub fn context_of(peer: Option<&str>, exchange: u64) -> wt::Context {
    wt::Context {
        peer: peer.map(str::to_owned),
        correlator: None,
        exchange,
    }
}

// ---------------------------------------------------------------------------
// Loading the artefact
// ---------------------------------------------------------------------------

/// A loaded component, its store and its host.
///
/// One instance answers every case: a case replaces the scripted deployment in
/// [`HostState`] before it calls, so a run is also evidence that the engine keeps no
/// state between exchanges it was not given.
pub struct Instance {
    /// The store, and the host state inside it.
    pub store: Store<HostState>,
    /// The three nlipsy exports, typed.
    pub engine: NlipsyHost,
    /// The Tokio runtime the outgoing `wasi:http` sender needs a reactor from.
    ///
    /// Held rather than entered: `wasmtime-wasi-http`'s default sender panics without
    /// one ("there is no reactor running"), and [`Instance::enter`] is what puts a guard
    /// around a call. Nothing of the corpus's is spawned onto it — `block_on` below is
    /// still the whole executor every export call is driven by.
    runtime: tokio::runtime::Runtime,
}

impl std::fmt::Debug for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance").finish_non_exhaustive()
    }
}

impl Instance {
    /// Load the artefact named by `NLIPSY_COMPONENT`, or the one
    /// `cargo +nightly build -p nlipsy-component --profile component --target
    /// wasm32-wasip3` leaves behind.
    ///
    /// **Loud when it is absent.** A `component` build that silently skipped because
    /// nobody had built the artefact would report a conformance it never checked, which
    /// is the one failure mode a corpus cannot afford — the same argument the CBOR cases
    /// are *counted* as skipped for rather than passed.
    pub fn load() -> wasmtime::Result<Instance> {
        let path = artefact_path();
        if !path.exists() {
            wasmtime::bail!(
                "no component at {}.\n\
                 Build it with `cargo +nightly build -p nlipsy-component --profile \\\n\
                 \x20   component --target wasm32-wasip3`, or name another with \\\n\
                 \x20   {ARTEFACT_ENV}=<path>.\n\
                 `make check-component` does both, on the nightly it pins.",
                path.display()
            );
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let mut config = Config::new();
        // The world has five `async func` exports and four asynchronous imports, which
        // is the component model's own concurrency rather than a host convention.
        // Everything else in it is an ordinary call.
        config.wasm_component_model_async(true);
        let wasm_engine = Engine::new(&config)?;
        let component = Component::from_file(&wasm_engine, &path)?;

        let mut linker: Linker<HostState> = Linker::new(&wasm_engine);
        // The fifteen WASI **0.3** interfaces `std` brings on `wasm32-wasip3` —
        // `wasi:cli@0.3.0` and `wasi:clocks@0.3.0`, none of them attributable to this
        // contract, and no `wasi:io`, because 0.3 carries streams natively — plus
        // `wasi:random`, which the guest's correlator asks for. `p3` has no `_sync`
        // variant and could not have one: every 0.3 import is asynchronous by
        // specification.
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        // And `wasi:http`, which is the component's own binding in both directions now:
        // `wasi:http/types@0.3.0` for the `handler` export's resources and
        // `wasi:http/client@0.3.0` for `client.call`.
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
        NlipsyHost::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |host| host)?;

        let mut store = Store::new(&wasm_engine, HostState::new());
        // Instantiated in two steps rather than through `Nlipsy::instantiate`, because
        // the raw instance is what a host callback needs in order to re-enter the guest:
        // `Nlipsy` is a set of typed function handles and is not `Clone`, while
        // `wasmtime::component::Instance` is a `Copy` handle `Nlipsy::new` reads back.
        let instance = {
            let _guard = runtime.enter();
            block_on(linker.instantiate_async(&mut store, &component))?
        };
        let engine = NlipsyHost::new(&mut store, &instance)?;
        store.data_mut().instance = Some(instance);
        Ok(Instance {
            store,
            engine,
            runtime,
        })
    }

    /// Run `call` with a Tokio reactor in scope.
    ///
    /// Every export call goes through here, because the outgoing `wasi:http` sender
    /// panics without a reactor and a test that only sometimes had one would fail in a
    /// way that named the wrong thing.
    pub fn enter<T>(&mut self, call: impl FnOnce(&mut Store<HostState>, &NlipsyHost) -> T) -> T {
        let _guard = self.runtime.enter();
        call(&mut self.store, &self.engine)
    }

    /// The `protocol` interface.
    pub fn protocol(&self) -> &ProtocolGuest {
        self.engine.nlipsy_engine_protocol()
    }

    /// The `engine` interface.
    pub fn clause6(&self) -> &EngineGuest {
        self.engine.nlipsy_engine_engine()
    }

    /// The `client` interface.
    pub fn client(&self) -> &ClientGuest {
        self.engine.nlipsy_engine_client()
    }

    /// The scripted deployment, to configure before a call.
    pub fn host(&mut self) -> &mut HostState {
        self.store.data_mut()
    }

    /// Put the deployment back the way `HostState::new` left it.
    ///
    /// Called between corpus cases: one instance answers all 125, and a case that
    /// inherited the previous case's authority would pass or fail for the wrong reason.
    pub fn reset(&mut self) {
        let host = self.store.data_mut();
        host.name = "corpus".to_owned();
        host.config = default_config_of(false);
        host.rejection = default_rejection_of();
        host.http_options = default_http_options_of();
        host.endpoint = CaseEndpoint::default();
        host.layers = Vec::new();
        host.authority = CaseAuthority::default();
        host.transport = CaseTransport::default();
        host.sink = CaseSink::default();
        host.entered.clear();
        host.resolved.clear();
    }
}

/// Drive one future to completion on this thread.
///
/// The corpus's calls are `*_async` and there is nothing to be asynchronous *about*:
/// the store is an asynchronous one because four of the contract's imports are
/// `async func`, and every export the corpus reaches returns without ever touching them.
/// So this is the whole executor the runner needs, and it is here rather than as a
/// dependency because a corpus runner that pulled in a work-stealing runtime to call
/// `decode` would be measuring the runtime.
///
/// `park`/`unpark` rather than a spin: a future that reports `Pending` after waking its
/// own waker — which is how `yield_once` hands a component-model task back to the
/// scheduler — leaves the unpark token set, so the `park` below returns immediately.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Poll, Wake, Waker};

    struct Unpark(std::thread::Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

// ---------------------------------------------------------------------------
// The corpus, through the component
// ---------------------------------------------------------------------------

/// The built component, as an [`Implementation`] of the corpus.
#[derive(Debug)]
pub struct Corpus {
    instance: Instance,
    /// The exchange number the next case runs under.
    ///
    /// One per case rather than `0`, so that a case driving `engine.check` and
    /// `engine.finish` drives them as two steps of **one** exchange: the contract mints
    /// a fresh number for every `0` it is handed, and a host keying state on
    /// `cx.exchange` would see two.
    exchange: u64,
}

impl Corpus {
    /// Load the artefact and get ready to answer cases with it.
    pub fn load() -> wasmtime::Result<Corpus> {
        Ok(Corpus {
            instance: Instance::load()?,
            exchange: 0,
        })
    }

    /// Put the deployment back and take the next exchange number.
    fn begin(&mut self) -> u64 {
        self.instance.reset();
        self.exchange += 1;
        self.exchange
    }

    /// The component's own answers to everything a message case asks.
    ///
    /// Every one of the six is a call into the component — `validate`, `is-control`,
    /// `extract-text`, `encode` and `token-kind-of` beside the `decode` that produced
    /// `message`. Deriving any of them from the Rust crates instead would be this
    /// workspace marking its own paper.
    fn observe(&mut self, message: &wt::Message) -> Result<Observed, String> {
        let Instance { store, engine, .. } = &mut self.instance;
        let protocol = engine.nlipsy_engine_protocol();

        let violations = block_on(protocol.call_validate(&mut *store, message))
            .map_err(|trap| format!("`validate` trapped: {trap:?}"))?;
        let is_control = block_on(protocol.call_is_control(&mut *store, message))
            .map_err(|trap| format!("`is-control` trapped: {trap:?}"))?;
        let text = block_on(protocol.call_extract_text(&mut *store, message, None))
            .map_err(|trap| format!("`extract-text` trapped: {trap:?}"))?;
        let token_count = block_on(protocol.call_token_count(&mut *store, message))
            .map_err(|trap| format!("`token-count` trapped: {trap:?}"))?;
        let first_submessage_token =
            block_on(protocol.call_first_submessage_token(&mut *store, message))
                .map_err(|trap| format!("`first-submessage-token` trapped: {trap:?}"))?;
        let encoded = block_on(protocol.call_encode(&mut *store, message, wt::Wire::Json))
            .map_err(|trap| format!("`encode` trapped: {trap:?}"))?;
        let canonical_json = String::from_utf8(encoded)
            .map_err(|why| format!("`encode` on the JSON wire produced non-UTF-8 bytes: {why}"))?;

        // The `token` filter is a comparison on a field the contract carries, not a
        // rule (`wit/README.md` §4); the *classification* is `token-kind-of`, which is
        // one, and is asked of the component.
        let mut tokens = Vec::new();
        for submessage in &message.submessages {
            if submessage.format != wt::Format::Token {
                continue;
            }
            let kind = block_on(protocol.call_token_kind_of(&mut *store, submessage))
                .map_err(|trap| format!("`token-kind-of` trapped: {trap:?}"))?;
            let kind = match kind {
                Some(wt::TokenKind::Conversation) => "conversation",
                Some(wt::TokenKind::Authentication) => "authentication",
                None => "other",
            };
            tokens.push(serde_json::json!({
                "subformat": submessage.subformat,
                "content": canonical_content(&content_in(submessage.content.clone())),
                "kind": kind,
            }));
        }

        Ok(Observed {
            model: canonical_model(&message_in(message)),
            violations: violations.iter().map(violation).collect(),
            is_control,
            text,
            tokens,
            token_count,
            // The record the component returned, read structurally — the *rule* is
            // §5 ¶2's "a message that is a token carries one", and that ran inside the
            // component when it answered `first-submessage-token`.
            first_submessage_token: first_submessage_token.as_ref().map(observed_token_of),
            canonical_json,
        })
    }

    /// Configure the deployment from a case's `endpoint` block.
    fn script(&mut self, case: &ExchangeCase) -> Result<(), String> {
        let rejection = rejection_of(&case.endpoint.rejection)?;
        let host = self.instance.host();
        host.config = wt::Config {
            issue_conversation_token: case.endpoint.config.issue_conversation_token,
            issue_authentication_token: case.endpoint.config.issue_authentication_token,
            max_tokens_per_message: u32::try_from(case.endpoint.config.max_tokens_per_message)
                .unwrap_or(u32::MAX),
        };
        host.rejection = wt::Rejection {
            inbound: policy_out(&rejection.inbound),
            outbound: policy_out(&rejection.outbound),
        };
        host.authority =
            CaseAuthority::Scripted(Box::new(ScriptedAuthority::new(&case.endpoint.authority)));
        Ok(())
    }

    /// Run one `short_circuit` exchange case: the composed order, with the reply
    /// supplied by **a layer whose `before` answers**.
    ///
    /// This is the route a Rust deployment takes, and under the contract it is the only
    /// route there is: the host's `answer` returns a `message` and
    /// `engine.finish` is the sole producer of a `finished`, so a reply that skipped
    /// `finish` can only come from a layer that answered above it. `engine.serve-message`
    /// then runs `check`, asks layer 0, gets `answer(reply)`, never reaches the handler
    /// or `finish`, and runs `Conformity::close` over the reply.
    ///
    /// What `serve-message` reports is exactly what the case asserts: `served.reply` is
    /// the engine's `close` over the layer's message, and `served.findings` are
    /// `check`'s alone, because `finish` never ran. They are therefore compared as
    /// `inbound`, which is not a guess — it is the whole content of the shape.
    fn short_circuit_case(&mut self, case: &ExchangeCase) -> Outcome {
        let mut out = Differences::new();
        let exchange = self.begin();

        let Some(value) = &case.endpoint.reply else {
            return Outcome::Failed(vec![
                "a `short_circuit` case needs an `endpoint.reply`: it is what the layer \
                 answered"
                    .into(),
            ]);
        };
        let reply = match self.decode_value(&value.clone()) {
            Ok(reply) => reply,
            Err(why) => {
                return Outcome::Failed(vec![format!("endpoint.reply does not decode: {why}")]);
            }
        };
        let request = match self.decode_value(&case.request) {
            Ok(request) => request,
            Err(why) => return Outcome::Failed(vec![format!("request does not decode: {why}")]),
        };
        if let Err(why) = self.script(case) {
            return Outcome::Failed(vec![why]);
        }

        // **One** layer, and its `before` answers. `host-layers.count()` is therefore
        // `1`, which is what the guest reads before it composes the stack.
        self.instance.host().layers = vec![CaseLayer {
            label: "short-circuit".to_owned(),
            before: Before::Answer(Box::new(reply)),
            after_suffix: None,
        }];
        // And the handler is scripted with nothing, so an implementation that reached it
        // fails loudly rather than answering from a script that happened to match.
        self.instance.host().endpoint = CaseEndpoint::default();

        let served = self.instance.enter(|store, engine| {
            let clause6 = engine.nlipsy_engine_engine();
            block_on(store.run_concurrent(async |accessor| {
                clause6
                    .call_serve_message(accessor, request, context_of(None, exchange))
                    .await
            }))
        });

        match served {
            Err(trap) => out.push(format!("`engine.serve-message` trapped: {trap:?}")),
            Ok(Err(trap)) => out.push(format!("`engine.serve-message` trapped: {trap:?}")),
            Ok(Ok(served)) => match &served.outcome {
                wt::Outcome::Ok => {
                    if case.expect.outcome != "reply" {
                        out.push(format!(
                            "outcome: expected `{}`, the request was answered",
                            case.expect.outcome
                        ));
                    }
                    compare_reply(case, &message_in(&served.reply), &mut out);
                    compare_withheld(case, served.withheld, &mut out);
                    compare_findings(case, &tagged("inbound", &served.findings), &mut out);
                }
                wt::Outcome::Refused(rejected) => {
                    if case.expect.outcome != "refused" {
                        out.push(format!(
                            "outcome: expected `{}`, the request was refused",
                            case.expect.outcome
                        ));
                    }
                    compare_refusal(case, &refusal(rejected), &mut out);
                    compare_findings(case, &[], &mut out);
                }
                other => out.push(format!(
                    "outcome: `serve-message` answered {other:?}, which no `short_circuit` \
                     case expects"
                )),
            },
        }

        // The handler must not have been asked: the layer answered above it. A script
        // with no turns would refuse, so this is belt and braces — and it is the
        // property the case shape exists to record.
        if !self
            .instance
            .host()
            .entered
            .contains(&"before:short-circuit".to_owned())
        {
            out.push("the layer's `before` was never called".into());
        }
        if !self.instance.host().resolved.is_empty() {
            out.push(format!(
                "a short-circuited exchange unwound through {:?}; the answering layer's \
                 own `after` must not run",
                self.instance.host().resolved
            ));
        }

        if out.is_empty() {
            Outcome::Passed
        } else {
            Outcome::Failed(out)
        }
    }

    /// Decode one of a case's JSON objects through the component, as
    /// `conformance/README.md`'s algorithm has a runner do.
    fn decode_value(&mut self, value: &Value) -> Result<wt::Message, String> {
        let body = serde_json::to_vec(value).map_err(|why| why.to_string())?;
        let Instance { store, engine, .. } = &mut self.instance;
        match block_on(engine.nlipsy_engine_protocol().call_decode(
            &mut *store,
            &body,
            wt::Wire::Json,
        ))
        .map_err(|trap| format!("`decode` trapped: {trap:?}"))?
        {
            Ok(message) => Ok(message),
            Err(error) => Err(format!(
                "{} at {}: {}",
                parse_error_kind(error.kind),
                error.path,
                error.detail
            )),
        }
    }
}

impl Implementation for Corpus {
    fn message_case(&mut self, case: &Case) -> Outcome {
        let (bytes, wire) = match case.wire.as_str() {
            "json" => match &case.input {
                Some(input) => (input.as_bytes().to_vec(), wt::Wire::Json),
                None => return Outcome::Failed(vec!["a json case without `input`".into()]),
            },
            // Never skipped, unlike the Rust run without its `cbor` feature: the
            // component is built with the codec, because ECMA-432 §7.1 is one of the two
            // cases of `types.wire` and a component that had only one would not implement
            // the contract.
            "cbor" => match &case.input_hex {
                Some(hex) => match crate::from_hex(hex) {
                    Ok(bytes) => (bytes, wt::Wire::Cbor),
                    Err(why) => return Outcome::Failed(vec![why]),
                },
                None => return Outcome::Failed(vec!["a cbor case without `input_hex`".into()]),
            },
            other => return Outcome::Failed(vec![format!("unknown wire `{other}`")]),
        };

        let decoded = {
            let Instance { store, engine, .. } = &mut self.instance;
            match block_on(
                engine
                    .nlipsy_engine_protocol()
                    .call_decode(&mut *store, &bytes, wire),
            ) {
                Ok(decoded) => decoded,
                // A trap is a failure of the *component*, not of the case: peer input
                // must not be able to take an instance down (`wit/nlipsy.wit`, `decode`).
                Err(trap) => return Outcome::Failed(vec![format!("`decode` trapped: {trap:?}")]),
            }
        };

        let differences = match (case.expect.decode.as_str(), decoded) {
            ("error", Err(error)) => compare_description(case, &described(&error)),
            ("error", Ok(message)) => vec![format!(
                "decode: expected an error, got a message ({})",
                crate::compact(&canonical_model(&message_in(&message)))
            )],
            ("ok", Err(error)) => vec![format!(
                "decode: expected a message, got {} at {}: {}",
                parse_error_kind(error.kind),
                error.path,
                error.detail
            )],
            ("ok", Ok(message)) => match self.observe(&message) {
                Ok(observed) => compare_observed(case, &observed),
                Err(why) => vec![why],
            },
            (other, _) => vec![format!("unknown expect.decode `{other}`")],
        };

        if differences.is_empty() {
            Outcome::Passed
        } else {
            Outcome::Failed(differences)
        }
    }

    fn exchange_case(&mut self, case: &ExchangeCase) -> Outcome {
        if case.kind != "exchange" {
            return Outcome::Failed(vec![format!("unknown case kind `{}`", case.kind)]);
        }
        if let Some(why) = malformed_case(case) {
            return Outcome::Failed(vec![why]);
        }
        if case.endpoint.short_circuit {
            return self.short_circuit_case(case);
        }
        let mut out = Differences::new();
        let exchange = self.begin();

        let request = match self.decode_value(&case.request) {
            Ok(request) => request,
            Err(why) => return Outcome::Failed(vec![format!("request does not decode: {why}")]),
        };
        if let Err(why) = self.script(case) {
            return Outcome::Failed(vec![why]);
        }

        // The end-point is never asked anything: an exchange case is `check` then
        // `finish`, which is what `conformance/README.md` steps 3 and 4 prescribe and the
        // only way to attribute a finding to a direction. Both steps run under the *same*
        // `cx.exchange`, so the scripted authority is asked about one exchange.
        let mut findings: Vec<Value> = Vec::new();
        let checked = {
            let Instance { store, engine, .. } = &mut self.instance;
            block_on(engine.nlipsy_engine_engine().call_check(
                &mut *store,
                &request,
                &context_of(None, exchange),
            ))
        };

        match checked {
            Err(trap) => out.push(format!("`engine.check` trapped: {trap:?}")),
            Ok(Err(rejected)) => {
                if case.expect.outcome != "refused" {
                    out.push(format!(
                        "outcome: expected `{}`, the request was refused",
                        case.expect.outcome
                    ));
                }
                compare_refusal(case, &refusal(&rejected), &mut out);
            }
            Ok(Ok(inbound)) => {
                if case.expect.outcome != "reply" {
                    out.push(format!(
                        "outcome: expected `{}`, the request was accepted",
                        case.expect.outcome
                    ));
                }
                findings.extend(tagged("inbound", &inbound));

                match &case.endpoint.reply {
                    None => out.push("expect.outcome is `reply` with no `endpoint.reply`".into()),
                    Some(value) => match self.decode_value(&value.clone()) {
                        Err(why) => out.push(format!("endpoint.reply does not decode: {why}")),
                        Ok(reply) => {
                            let finished = {
                                let Instance { store, engine, .. } = &mut self.instance;
                                block_on(engine.nlipsy_engine_engine().call_finish(
                                    &mut *store,
                                    &request,
                                    &reply,
                                    &context_of(None, exchange),
                                ))
                            };
                            match finished {
                                Err(trap) => out.push(format!("`engine.finish` trapped: {trap:?}")),
                                Ok(finished) => {
                                    findings.extend(tagged("outbound", &finished.findings));
                                    compare_reply(case, &message_in(&finished.message), &mut out);
                                    compare_withheld(case, finished.withheld, &mut out);
                                }
                            }
                        }
                    },
                }
            }
        }

        compare_findings(case, &findings, &mut out);

        if out.is_empty() {
            Outcome::Passed
        } else {
            Outcome::Failed(out)
        }
    }
}

// ---------------------------------------------------------------------------
// The binding: `wasi:http/handler@0.3.0`, driven as a peer drives it
// ---------------------------------------------------------------------------

/// What one HTTP exchange through the component's own binding produced.
#[derive(Debug, Clone)]
pub struct HttpAnswer {
    /// The status line.
    pub status: u16,
    /// Every header, lower-cased, in the order the guest appended them.
    pub headers: Vec<(String, String)>,
    /// The body, as bytes.
    pub body: Vec<u8>,
}

impl HttpAnswer {
    /// The first value of `name`, or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body as text, lossily — every reply this binding writes is JSON.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl Instance {
    /// Drive one request through the component's `wasi:http/handler@0.3.0` export.
    ///
    /// `wasmtime_wasi_http::p3::bindings::Service::new` finds the export on the instance
    /// the store already holds, so there is no binding of this host's for it and no
    /// `add_to_linker` — the runtime provides `wasi:http` and the component *is* the
    /// server. What goes in is an ordinary
    /// `http::Request`, which is what a peer sends.
    pub fn http(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> wasmtime::Result<HttpAnswer> {
        use http_body_util::BodyExt as _;
        use wasmtime_wasi_http::p3::bindings::Service;

        let _guard = self.runtime.enter();

        let instance = self
            .store
            .data()
            .instance
            .ok_or_else(|| wasmtime::Error::msg("the host has no instance handle"))?;
        let service = Service::new(&mut self.store, &instance)?;

        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let outgoing = builder.body(http_body_util::Full::new(bytes::Bytes::from(body)))?;
        let (request, _sent) = wasmtime_wasi_http::p3::Request::from_http(
            wasmtime_wasi_http::default_hooks(),
            outgoing,
        );

        let response = match block_on(
            self.store
                .run_concurrent(async |accessor| service.handle(accessor, request).await),
        )?? {
            Ok(response) => response,
            Err(code) => {
                return Err(wasmtime::Error::msg(format!(
                    "the guest answered `wasi:http` with an error code rather than a \
                     response: {code:?}"
                )));
            }
        };

        let status = response.status.as_u16();
        let headers = response
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body: Vec<u8> = block_on(self.store.run_concurrent(
            async |accessor| -> wasmtime::Result<Vec<u8>> {
                let finished = std::future::ready(Ok::<(), wasmtime_wasi_http::Error>(()));
                let answered = accessor.with(|store| response.into_http(store, finished))?;
                let collected = answered
                    .into_body()
                    .collect()
                    .await
                    .map_err(|why| wasmtime::Error::msg(format!("{why:?}")))?;
                Ok(collected.to_bytes().to_vec())
            },
        ))??;

        Ok(HttpAnswer {
            status,
            headers,
            body,
        })
    }

    /// Drive one request whose body stops early, the way a peer that hangs up mid-send
    /// does.
    ///
    /// `prefix` reaches the guest as a data frame and the stream then fails, which is
    /// what a runtime reports through `consume-body`'s second value — the
    /// `future<result<option<trailers>, error-code>>` that is the only place
    /// `wasi:http@0.3.0` says a body was not fully received. There is no way to reach
    /// that path with an ordinary `http_body_util` body: `Full` never fails and `Limited`
    /// refuses a whole frame rather than truncating one, so the shape is spelt out in
    /// `BrokenBody`.
    pub fn http_broken_body(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        prefix: Vec<u8>,
    ) -> wasmtime::Result<HttpAnswer> {
        use http_body_util::BodyExt as _;
        use wasmtime_wasi_http::p3::bindings::Service;

        let _guard = self.runtime.enter();

        let instance = self
            .store
            .data()
            .instance
            .ok_or_else(|| wasmtime::Error::msg("the host has no instance handle"))?;
        let service = Service::new(&mut self.store, &instance)?;

        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let outgoing = builder.body(BrokenBody {
            prefix: Some(bytes::Bytes::from(prefix)),
        })?;
        let (request, _sent) = wasmtime_wasi_http::p3::Request::from_http(
            wasmtime_wasi_http::default_hooks(),
            outgoing,
        );

        let response = match block_on(
            self.store
                .run_concurrent(async |accessor| service.handle(accessor, request).await),
        )?? {
            Ok(response) => response,
            Err(code) => {
                return Err(wasmtime::Error::msg(format!(
                    "the guest answered `wasi:http` with an error code rather than a \
                     response: {code:?}"
                )));
            }
        };

        let status = response.status.as_u16();
        let headers = response
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body: Vec<u8> = block_on(self.store.run_concurrent(
            async |accessor| -> wasmtime::Result<Vec<u8>> {
                let finished = std::future::ready(Ok::<(), wasmtime_wasi_http::Error>(()));
                let answered = accessor.with(|store| response.into_http(store, finished))?;
                let collected = answered
                    .into_body()
                    .collect()
                    .await
                    .map_err(|why| wasmtime::Error::msg(format!("{why:?}")))?;
                Ok(collected.to_bytes().to_vec())
            },
        ))??;

        Ok(HttpAnswer {
            status,
            headers,
            body,
        })
    }
}

/// A request body that hands over a prefix and then fails.
///
/// One data frame, then `Err`, which is a peer that declared a length and stopped
/// sending. The runtime turns that into the `error-code` the guest reads off the
/// trailers future; everything else about the exchange is ordinary.
#[derive(Debug)]
struct BrokenBody {
    prefix: Option<bytes::Bytes>,
}

impl http_body::Body for BrokenBody {
    type Data = bytes::Bytes;
    type Error = wasmtime_wasi_http::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        match self.prefix.take() {
            Some(prefix) if !prefix.is_empty() => {
                std::task::Poll::Ready(Some(Ok(http_body::Frame::data(prefix))))
            }
            // The connection died. `ConnectionTerminated` is what a runtime reports for a
            // peer that stopped writing, and the guest reads it as
            // `error-code.connection-terminated`.
            _ => std::task::Poll::Ready(Some(Err(wasmtime_wasi_http::Error::ConnectionTerminated))),
        }
    }
}

// ---------------------------------------------------------------------------
// The composed artefact: the engine plugged into a Rust end-point
// ---------------------------------------------------------------------------

/// Where `wac plug` leaves the composition when nobody says otherwise.
const DEFAULT_COMPOSED: &str = "../../target/wasm32-wasip3/component/nlipsy_echo.wasm";

/// The environment variable that overrides it.
pub const COMPOSED_ENV: &str = "NLIPSY_COMPOSED";

/// Where the composed artefact is.
pub fn composed_path() -> PathBuf {
    match std::env::var_os(COMPOSED_ENV) {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_COMPOSED),
    }
}

/// A host with **no scripted deployment at all**, for the composed artefact.
///
/// `examples/echo-endpoint` supplies the five `host-*` interfaces and `wac plug` joins
/// them to the engine's imports, so what is left imports only `wasi:*` — it is a
/// complete server, and the point of driving it from here is that this host contributes
/// nothing to the answer but a socket's worth of bytes. It is what `wasmtime serve`
/// serves, checked in-process so that `make check-component` does not need a port.
pub struct Composed {
    store: Store<Bare>,
    instance: wasmtime::component::Instance,
    runtime: tokio::runtime::Runtime,
}

impl std::fmt::Debug for Composed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composed").finish_non_exhaustive()
    }
}

/// WASI and nothing else — the whole of what a composed artefact asks a host for.
pub struct Bare {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
}

impl std::fmt::Debug for Bare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bare").finish_non_exhaustive()
    }
}

impl WasiView for Bare {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for Bare {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: default_hooks(),
        }
    }
}

impl Composed {
    /// Load the composition named by `NLIPSY_COMPOSED`, or the one `wac plug` leaves
    /// behind.
    ///
    /// **Loud when it is absent**, for the reason [`Instance::load`] is: a check that
    /// silently skipped would report a composition it never ran.
    pub fn load() -> wasmtime::Result<Composed> {
        let path = composed_path();
        if !path.exists() {
            wasmtime::bail!(
                "no composed artefact at {}.\n\
                 Build it with `wac plug <engine> --plug <end-point> -o <out>`, or name \\\n\
                 \x20   another with {COMPOSED_ENV}=<path>.\n\
                 `make check-component` does both.",
                path.display()
            );
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let engine = Engine::new(&config)?;
        let component = Component::from_file(&engine, &path)?;

        let mut linker: Linker<Bare> = Linker::new(&engine);
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
        // `nlipsy:engine/types@0.1.0` declares no functions — it is the shared vocabulary
        // and nothing else — so it costs the instance an empty import item that both
        // halves of the composition carry and neither exports. An empty instance is the
        // whole of what satisfying it means, and `wasmtime serve` does the same.
        linker.instance("nlipsy:engine/types@0.1.0")?;

        let mut store = Store::new(
            &engine,
            Bare {
                wasi: WasiCtxBuilder::new().inherit_stdio().build(),
                http: WasiHttpCtx::default(),
                table: ResourceTable::new(),
            },
        );
        let instance = {
            let _guard = runtime.enter();
            block_on(linker.instantiate_async(&mut store, &component))?
        };
        Ok(Composed {
            store,
            instance,
            runtime,
        })
    }

    /// Drive one request through the composition's `wasi:http/handler@0.3.0` export.
    ///
    /// The same road `wasmtime serve` takes, minus the socket.
    pub fn http(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> wasmtime::Result<HttpAnswer> {
        use http_body_util::BodyExt as _;
        use wasmtime_wasi_http::p3::bindings::Service;

        let _guard = self.runtime.enter();
        let service = Service::new(&mut self.store, &self.instance)?;

        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let outgoing = builder.body(http_body_util::Full::new(bytes::Bytes::from(body)))?;
        let (request, _sent) = wasmtime_wasi_http::p3::Request::from_http(
            wasmtime_wasi_http::default_hooks(),
            outgoing,
        );

        let response = match block_on(
            self.store
                .run_concurrent(async |accessor| service.handle(accessor, request).await),
        )?? {
            Ok(response) => response,
            Err(code) => {
                return Err(wasmtime::Error::msg(format!(
                    "the composition answered `wasi:http` with an error code rather than \
                     a response: {code:?}"
                )));
            }
        };

        let status = response.status.as_u16();
        let headers = response
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body: Vec<u8> = block_on(self.store.run_concurrent(
            async |accessor| -> wasmtime::Result<Vec<u8>> {
                let finished = std::future::ready(Ok::<(), wasmtime_wasi_http::Error>(()));
                let answered = accessor.with(|store| response.into_http(store, finished))?;
                let collected = answered
                    .into_body()
                    .collect()
                    .await
                    .map_err(|why| wasmtime::Error::msg(format!("{why:?}")))?;
                Ok(collected.to_bytes().to_vec())
            },
        ))??;

        Ok(HttpAnswer {
            status,
            headers,
            body,
        })
    }
}
