# nlipsy-core

[![crates.io](https://img.shields.io/crates/v/nlipsy-core.svg)](https://crates.io/crates/nlipsy-core)
[![docs.rs](https://img.shields.io/docsrs/nlipsy-core)](https://docs.rs/nlipsy-core)
[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)
[![licence](https://img.shields.io/crates/l/nlipsy-core.svg)](https://github.com/laktoosivaba/nlipsy#licence)

The NLIP runtime core: clause 6 of ECMA-430 as values and traits, with no opinion
about execution.

`nlipsy-protocol` says what a *message* is (clause 5). This crate says what an
*end-point* owes a peer that sent one (clause 6) — echo its tokens, answer a control
request in kind, refuse a reply that breaks clause 5 — and what a *client agent* does
to send one (§4.7). It does none of it for you: there is no accept loop, no socket, no
executor and no clock here. A binding crate supplies those.

## The boundary, in three sentences

One test decides what belongs here: does it compile for `wasm32-unknown-unknown`, and
can it be tested without an async executor? Clause 6 passes — its steps (`check`
before the handler, `finish` after it, `echo_into` on the failure path and `close` over
a reply `finish` never saw) are synchronous functions over values, and everything they need from a deployment arrives
through two traits and a type map. Anything that opens a socket, spawns a task, reads
a clock or touches a file is a *runtime* and lives in `nlipsy-http-client`,
`nlipsy-server`, or the deployment itself.

## What is here

| item | feature | what it is |
|---|---|---|
| `Context` | — | the per-exchange value a runtime fills: `peer`, `correlator`, `http::Extensions` |
| `MaybeSend`, `BoxFuture` | — | `Send` on a target with threads, nothing on `wasm32` |
| `Endpoint`, `DynEndpoint`, `EndpointError` | `server` | the handler seam: a message in, a message or a failure out |
| `TokenAuthority`, `TokenKind` | `server` | §6.2's exception — "did this end-point create this token?" — and §6.2.1/§6.2.2's keeping and minting |
| `Conformity`, `ConformityConfig`, `Rejection` | `server` | clause 6 itself, as synchronous steps and one wrapper |
| `serve`, `serve_message`, `serve_answered`, `Served`, `Outcome` | `server` | the *sequence* a binding drives around those steps, and how one exchange ended |
| `owed`, `Owed`, `error_reply` | `server` | the failure rules: what a refusal still owes the peer, and the reply it goes back as |
| `Middleware`, `DynMiddleware`, `Next` | `server` | the message-level layer seam: a stack between clause 6's check and the endpoint, with `&mut Context` |
| `Dispatcher` | `server` | §5.1.1's two-way split — control to one endpoint, everything else to another |
| `Transport`, `DynTransport`, `Exchange` | `client` | the outbound seam: bytes out, bytes back |
| `Client`, `Reply`, `Body`, `encode_request`, `decode_reply` | `client` | one exchange, and what to make of what came back |
| `Recorded`, `Recorder`, `Replay`, `RecordSink` | `client` | real wires, kept so tests need no network |
| `testing::Scripted` | `testing` | a transport that answers from a script |
| — | `cbor` | forwards to `nlipsy-protocol/cbor`, so `Wire::Cbor` can be encoded |

No default features: `nlipsy-core = "0.1"` on its own compiles the shared items and
nothing else. An endpoint takes `server`, a client agent takes `client`, and a
deployment that federates takes both — which is how `jsonrpsee-core` splits, for the
same reason.

## Clause 6, around a trivial endpoint

The steps are synchronous, so this runs with no executor at all:

```rust
# // The example is the `server` half. Hidden `cfg`s rather than `ignore`, so that
# // `cargo test` with the feature on really compiles and runs it.
# #[cfg(not(feature = "server"))]
# fn main() {}
# #[cfg(feature = "server")]
# fn main() {
use nlipsy_core::{Conformity, ConformityConfig, Context, Rejection, TokenAuthority, TokenKind};
use nlipsy_protocol::{Message, Submessage};
use std::sync::Mutex;

// The endpoint. It answers, and knows nothing about tokens.
struct Greeter;

// An in-memory authority: one conversation token, minted once and kept.
#[derive(Default)]
struct Memory(Mutex<Option<String>>);

impl TokenAuthority for Memory {
    fn is_own(&self, token: &str, _cx: &Context) -> bool {
        self.0.lock().unwrap().as_deref() == Some(token)
    }

    // A read, with no side effect. §6.2.1 obliges the end-point that created a
    // conversation token to keep confirming it, so this is what puts it in every reply.
    fn issued(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    // A write, called only when `issued` returned `None` and the peer brought no
    // identifier of its own. Remembering it is this method's job.
    fn mint(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
        let mut held = self.0.lock().unwrap();
        *held = Some("conv-1".to_string());
        held.clone()
    }
}

let clause6 = Conformity::new(
    Greeter,
    Memory::default(),
    ConformityConfig::new().with_conversation_token(true),
    Rejection::default(),
);
let cx = Context::new().with_peer("10.0.0.1").with_correlator("c-1");

// First exchange: the peer brings nothing, so the endpoint issues an identifier.
let hello = Message::text("Hello?");
clause6.check(&hello, &cx).expect("the request conforms");
let first = clause6.finish(&hello, Message::text("Hello."), &cx).message;
assert_eq!(first.tokens().next().unwrap().content_as_str(), Some("conv-1"));

// Second exchange: the peer returns it, and asks a control question.
let again = Message::text("Still there?")
    .message_type("Control")
    .with_submessage(Submessage::token("conversation", "conv-1"))
    .with_submessage(Submessage::token("authorization", "the-peer's-own"));

let findings = clause6.check(&again, &cx).expect("the request conforms");
assert!(findings.is_empty());
let second = clause6.finish(&again, Message::text("Still here."), &cx).message;

// §6.2 — the peer's own token comes straight back, whatever it is for; the
// endpoint's own is not echoed, because §6.2's exception is about authorship — it
// travels back once as the conversation's identifier instead, through `issued`.
let returned: Vec<_> = second.tokens().map(|t| t.content_as_str().unwrap()).collect();
assert_eq!(returned, ["the-peer's-own", "conv-1"]);
// §6.3 — a control request got a control answer, in the spelling the peer used.
assert_eq!(second.message_type.as_deref(), Some("Control"));
# }
```

`Conformity` also implements `Endpoint` — `check`, then the inner endpoint, then
`finish`, one `await` and no task — which is what a runtime wraps a handler in. The
steps are public because a binding that has to put a timeout around the handler alone,
or that answers a failure before the handler ran, needs them and not the wrapper.

## The composer a binding drives

`Conformity` holds the obligations; `nlipsy_core::serve` holds the **order they run
in**, and that order is the other half of being an end-point:

```text
decode → check → [ layers … [ endpoint → finish ] ] → close → Served
                  ↑ check is outside the stack            ↑ unconditionally
```

It returns `Served { reply, outcome, withheld, findings }` and never an error: every
way an exchange can end — `Ok`, `Malformed`, `Refused`, `Failed` — produces a message
the peer can read, because a binding that has to invent a body for a failure will
invent a different one from the next binding. `Outcome::public_message` is the text the
peer may see and `Outcome::retry_after` the delay a dependency named; `owed` and
`error_reply` are the same rules as free functions, for a refusal a binding produces
on its own (a rate limiter that has already decoded the request).

`serve_answered` is the same order with the middle supplied by the caller: `check`, a
future that answers with a `Finished`, `close` over whatever came back, and the same
failure rules. `serve_message` *is* that function with the layer stack as its answer, so
the order exists once. It is for a binding that owns its own layer stack and ran `finish`
itself, so that what comes back has already been finished and must be *closed* rather
than finished a second time: minting is in `finish`, and a second pass under an authority
that has not yet been told what the first mint was hands the peer two identifiers. The
only way to hold such a value is to have called `Conformity::finish` — there is no
constructor for one, so no caller outside this crate can build a `Finished`, and the
WebAssembly guest composes its layers through `Middleware` like any other binding
rather than finishing a reply by hand.

Two properties are the reason this is code and not a paragraph in a README.
`Conformity::finish` runs at the **bottom** of the layer stack, because §6.2.1's token
is minted there and a deployment that persists what it minted has to run after it.
`Conformity::close` runs **after** the stack, unconditionally, because a layer may
answer without ever reaching the bottom. A second binding — ECMA-432's frames, the
`wasi:http` handler in the component over the same engine — would otherwise re-derive
the order and get one of the two wrong.

What stays outside: reading the bytes, choosing a status code or a frame header,
logging, and the clock. `Conformity`'s own `Endpoint` implementation is unchanged and
remains the one-`await` convenience for a caller that wants clause 6 around a handler
and nothing else.

A binding whose layers can answer without reaching the handler — a cache, a canned
answer, a maintenance-mode layer — or can edit the reply afterwards runs
`Conformity::close` over whatever came back: the §6.2 echo, the §6.2.1 confirmation and
the §6.3 carry are the three steps that are idempotent, so applying them again costs
three linear scans and restores what a layer removed. Nothing a layer does can remove
what clause 6 owes the peer; a layer can still break clause 5 after `finish`, and the
outbound policy does not see it. `close` never mints, because minting is a write and
the paths that need `close` are the ones a deployment does not commit.

## What is deliberately not here

- **No runtime.** No tokio, no `tracing`, no `reqwest`, no `axum`, no time in any
  form. Findings come back as data and the runtime decides what to log; a
  `Retry-After` is a `Duration` on a value and the binding decides how to spell it.
- **No session.** §6.2 makes conversation state optional — "if the end-points are
  maintaining session state" — so a core that named a `Session` type would have
  settled for every deployment a question the standard leaves open. A deployment puts
  its own in `Context::extensions` and reads it back in its `TokenAuthority`.
- **No binding.** The `/nlip` path is ECMA-431 §6's, `X-NLIP-Correlator` is a
  convention of one reference server, CBOR framing is ECMA-432 §7.1's, and a status
  code is a choice ECMA-431 leaves open. Each lives with the crate that speaks it —
  `nlipsy-http` for the HTTP ones, shared by both ends of that binding. What is here is
  the part all three share: `serve` names an `Outcome`, and the binding maps it.
- **No file I/O.** `Recorder` writes to a `RecordSink`, not to a path, so the crate
  keeps compiling for `wasm32` and a browser can record into whatever it has.
- **No second reply.** One reply per exchange. ECMA-433 §6.2's several replies —
  "responses in each of the indicated content-types" — are one finished message,
  rendered by the AMQP binding once per requested content type and addressed per
  `nlip-reply-<content-type>`; the tokens and the control carry are the same in each. A
  reply whose *content* differs per format (§6.2's own example: JSON for a machine and
  text for a person) is not expressible through `Endpoint` and is a decision for that
  binding, not a promise of this one.
- **No decode step on the server side.** `Conformity::check` and `Endpoint::handle`
  both take a `&Message`, so a binding decodes the wire itself and holds the choice
  ECMA-432 §11 gives it — "if CBOR decoding fails, a server end-point shall send back a
  NLIP error message using the fallback endpoint" — before the core has been asked
  anything. That is an omission by design and not an oversight.
- **No proc-macros.** There is nothing to generate: NLIP has no method names, no
  request ids, no batches and no subscriptions, so a macro here would emit an
  `impl Endpoint` with one method — which is the trait, written out.

## Readings

Where clause 6 is open, the reading this crate takes is written here rather than left
in the code. Each is pinned by a case under `conformance/cases/readings/`, so a second
implementation can disagree in the file instead of in its code.

**A replacement error message in a control exchange keeps `MessageType: Error`** and
satisfies §6.3 by the boolean alone. §6.3 names "control field set to true" and no
other field; §5.1.1 gives `MessageType` one value, so a reply cannot spell both
`control` and `Error` there, and `Error` is the one in-band marker for a failure
(TR/113 §10, ECMA-432 §11, `Reply::is_error`). A peer that reads only `MessageType`
sees an error and not a control reply; a peer that reads §6.3's field sees both. A
handler that answers a control request with `Message::error(..)` produces the same shape
on the happy path, so the two paths agree.

**An explicit `control: false` from the handler is overruled.** §6.3 carries no
exception and "the end-point" is the whole stack from the peer's side, so the boolean is
overwritten rather than filled. `messagetype` is filled only: the clause names the
boolean and nothing else, and a handler's own value is its answer's classification.

**§6.2.1's option to mint beside the peer's token is exercised only when the peer
brought none.** §6.2.1 lets the other agent add its own conversation token beside the
peer's, and §6.2 ¶3's example is two conversation tokens in one message. This engine
declines the option when the peer named the conversation first, so that a deployment
keying its state on the peer's identifier is not handed a second one to store — a
choice the clause leaves open, not a rule. A deployment that wants its own identifier
beside the peer's mints it in its handler, and its `TokenAuthority` recognises it as
own.

**One reply per exchange, rendered per content type.** ECMA-433 §6.2's "responses in
each of the indicated content-types" are one finished message rendered several times,
not several answers; see *What is deliberately not here*.

## `MaybeSend`, and why

Every trait here is bounded by `MaybeSend` rather than `Send`: it *is* `Send` on a
target with threads and nothing on `wasm32`, where a JavaScript callback is not `Send`
and never will be. The marker is copied from jsonrpsee's `core/src/client/mod.rs`,
which solves the same problem for the same target. `Sync` is kept as a real bound —
an endpoint is shared behind an `Arc` and answers several requests at once — and on a
single-threaded target it costs nothing.

## Where this crate sits

Six published crates, in the order they depend on each other:

| crate | is |
|---|---|
| [`nlipsy-protocol`](https://crates.io/crates/nlipsy-protocol) | ECMA-430 clause 5 — what a message *is* |
| [`nlipsy-core`](https://crates.io/crates/nlipsy-core) | ECMA-430 clause 6 — what an end-point *owes* a peer, and the sequence a binding drives, with no runtime in it |
| [`nlipsy-http`](https://crates.io/crates/nlipsy-http) | ECMA-431 §6 as values — the status map, the header spellings, `Retry-After` both ways |
| [`nlipsy-http-client`](https://crates.io/crates/nlipsy-http-client) | ECMA-431 from a client agent's end — one `POST`, over reqwest |
| [`nlipsy-server`](https://crates.io/crates/nlipsy-server) | ECMA-431 from a server agent's end — one `axum::Router`, clause 6 underneath |
| [`nlipsy`](https://crates.io/crates/nlipsy) | the facade: all of them as modules behind feature flags |

Depending on any of them directly is supported and always will be. The facade is for
the consumer's manifest, not for their imports: `nlipsy = { version = "0.1", features =
["client-core"] }` is this crate's outbound half and `["server-core"]` its inbound one,
and the module is `nlipsy::core` either way.

## Minimum supported Rust version

1.85, the edition 2024 floor. Raising it is treated as a breaking change, and it is
checked rather than asserted: `cargo +1.85 check --workspace --all-targets
--all-features` is part of what a release runs.

## Licence

Licensed under either of

- Apache License, Version 2.0
  ([LICENSE-APACHE](https://github.com/laktoosivaba/nlipsy/blob/master/LICENSE-APACHE)
  or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence
  ([LICENSE-MIT](https://github.com/laktoosivaba/nlipsy/blob/master/LICENSE-MIT)
  or <https://opensource.org/licenses/MIT>)

at your option.

The links are spelled as full URLs rather than as `[LICENSE](LICENSE)`: this file is
the crate documentation, and rustdoc reads a relative link with no extension as an
intra-doc link to a Rust item — one that does not exist, which is a warning on every
build.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 licence, shall be
dual-licensed as above, without any additional terms or conditions.

ECMA-430, ECMA-431, ECMA-432, ECMA-433, ECMA-434 and TR/113 are publications of Ecma
International and are not redistributed here.
