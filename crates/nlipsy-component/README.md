# nlipsy-component

The `wit-bindgen` guest for [`wit/nlipsy.wit`](../../wit/nlipsy.wit): the nlipsy engine
as a WebAssembly component. It composes the host's layers over the host's handler, it
carries ECMA-431 §6 itself through `wasi:http/handler@0.3.0`, and it is the client too
through `wasi:http/client@0.3.0`. What a host supplies is **code** — a handler, layers, a
token authority, an alternative transport, somewhere to record — and nothing else
([`wit/README.md`](../../wit/README.md), *The design*).

**No resource crosses the boundary in either direction.** The host's five interfaces are
plain functions and `types.context` is a record by value carrying an `exchange` number a
host keys its own state on, so nothing is borrowed, nothing leaks, and this crate has no
hand-written `unsafe` at all.

It is **not published**. What a consumer installs is the npm package in
[`javascript/`](../../javascript/), built from the artefact; this crate is the
artefact's source, and it contains no rule of
its own. Every export is one of three crates' — `nlipsy-protocol` for ECMA-430 clause 5,
`nlipsy-core` for clause 6 and the composed order around it, `nlipsy-http` for ECMA-431
§6 as values — re-spelled in the contract's vocabulary. `wit/README.md` §3 is the seed
table, one row per item, and nothing may be exported here that has no row in it.

## The modules

| file | what it is |
|---|---|
| `src/host.rs` | the five imports behind the engine's own traits: `HostEndpoint` is an `Endpoint`, `HostLayer` is a `Middleware` per layer index, `HostAuthority` is a `TokenAuthority`, `HostTransport` is a `Transport`, `HostSink` buffers for a `RecordSink` |
| `src/engine.rs` | `export interface engine` — clause 6's two steps, the composed order over both, and the three rules over an `outcome` |
| `src/client.rs` | `export interface client` — `call` over `wasi:http`, `call-through` over the host's transport, `call-recording`, `replay` |
| `src/http.rs` | `export wasi:http/handler@0.3.0` — the ECMA-431 §6 binding, `nlipsy-server`'s behaviour transposed |
| `src/protocol.rs` | `export interface protocol` — ECMA-430 clause 5, as pure functions |
| `src/convert.rs` | the contract's vocabulary and the engine's, in both directions |

## Building it

```text
rustup toolchain install nightly-2026-09-15
rustup target add wasm32-wasip3 --toolchain nightly-2026-09-15
cargo +nightly-2026-09-15 build -p nlipsy-component --profile component --target wasm32-wasip3
# => target/wasm32-wasip3/component/nlipsy_component.wasm
wasm-tools validate --features all target/wasm32-wasip3/component/nlipsy_component.wasm
wasm-tools component wit          target/wasm32-wasip3/component/nlipsy_component.wasm
```

`make check-component` is those lines followed by the composition with
[`examples/echo-endpoint`](../../examples/echo-endpoint/), the conformance corpus run
through the artefact in wasmtime, and the HTTP-level tests through `wasi:http/handler` —
with the toolchain in one variable (`COMPONENT_TOOLCHAIN`). `make check` does not build
it, because the toolchain, the target, `wasm-tools`, `wac` and wasmtime are five things a
fresh checkout does not have.

The artefact on its own imports the five `host-*` interfaces and cannot run alone. What
runs alone is the composition:

```text
wac plug target/wasm32-wasip3/component/nlipsy_component.wasm \
    --plug target/wasm32-wasip3/component/echo_endpoint.wasm \
    -o target/wasm32-wasip3/component/nlipsy_echo.wasm
make serve-example
```

**Why a nightly, and until when.** `wasm32-wasip3` is where this component is built: its
`std` links WASI **0.3** and nothing else, which is what the artefact's import list says —
`wasi:cli@0.3.0` and `wasi:clocks@0.3.0`, fifteen interfaces, with no `wasi:io` at all,
because 0.3 carries streams natively. `rust-std` for the target ships on nightly today
and on **stable at Rust 1.100, 2026-11-12**; on that day `COMPONENT_TOOLCHAIN` becomes
`stable` and this section loses its first line. The crate's own `rust-version` stays
where it is — it is what the *source* needs, and the `rlib` half of it is compiled and
tested by `make check` on stable.

`cargo build --target wasm32-wasip3` emits a **component**, with no `cargo-component`,
no `wasm-tools component new` and no adapter of any kind. The `component` profile is
declared in the workspace root, because Cargo reads `[profile.*]` there and nowhere
else; it inherits `release` and adds the size settings the root manifest explains, one
by one.

`wit_bindgen::generate!` is given `generate_all` and no `with:` map. The world reaches
`wasi:http@0.3.0`, which pulls `wasi:clocks/types@0.3.0` in transitively, and wit-bindgen
otherwise stops with `missing one of: * generate_all option * with: { … }` because it
assumes WASI packages come from a crate. A `with:` map onto `wasip3`'s pregenerated
bindings would make this crate depend on a crate for bindings it can generate itself from
the WIT already in the tree: the *target* decides what `std` links, `wit/deps/` decides
what the component asks for.

The crate is also an `rlib`, which is what lets `cargo test -p nlipsy-component` run on
the host: wit-bindgen's generated code compiles for a native target with every
canonical-ABI import replaced by a stub, so the conversions and the context projection
are testable without a wasm runtime.

Its `rust-version` is its own and is past the workspace's 1.85, because wit-bindgen 0.62
and edition 2024 are. Every 1.85 check excludes this member by name, and says so at the
exclusion; the six published crates are unaffected.

## The composed order, which is the point of this crate

`nlipsy_core::serve` is what runs, and nothing here re-derives it: decode, `check`
*outside* the stack, the host's layers outermost first, `answer` at the bottom with
`Conformity::finish` under it, the layers back innermost first, `Conformity::close` over
whatever came back — unconditionally — and on any failure the rules of `owed` and
`error_reply` instead.

Two of those are properties rather than steps. `finish` is at the **bottom** because
§6.2.1's token is minted there and a deployment that persists what it minted has to run
after that, so its layer's after-half must see the reply that goes on the wire. `close`
runs **after** the stack because a layer may answer without ever reaching the bottom —
and that is what `host-layers.before` returning `layer-step.answer` is: the handler and
every layer below are not run, the answering layer's own `after` is not called, and the
three idempotent obligations are re-applied to the reply anyway. `conformance/cases/
readings/51`–`/55` are the five cases that pin it.

The seam the contract had to split is `Middleware` and `Next`: a Rust layer is one
"around" call that receives `next` and decides whether to run it, and `next` cannot cross
a boundary. So each layer is addressed by index and answers two questions instead of one,
and `src/host.rs` is where the two halves become one call again.

## What outlives a call: one number

Nothing else. The `Conformity` every exported call runs under is built *per call* from
`host-endpoint.conformity`, `host-endpoint.rejection-policy` and the `host-authority`
import, so nothing in this guest is `'static` that need not be — which is the reason
there is no `unsafe` here.

The one exception is the exchange counter: a `static AtomicU64` in `src/host.rs` that
hands out `types.context`'s `exchange` number when a caller passes `0`. It identifies an
exchange **within one component instance** and nowhere else — it is not a correlator, it
is never on a wire, and it means nothing to a second instance. A host keys its session,
its deadline or its authenticated principal on it and reads it back in its authority, its
layers and its handler; `Context::extensions` is the Rust type map that number replaces,
and this guest puts nothing else in it.

## What the guest may never do

Peer input must not trap the instance: a component that panicked would take down an
instance that may be answering other exchanges concurrently, and `panic = "abort"` means
there is no unwinding to catch it. So nothing here calls `unwrap`, `expect` or an index
on a host-supplied value, every count that narrows is saturating, and every conversion
from the contract into the engine is total — including `content.structured`, whose
defined answer for a string that is not JSON is to read it as `text` (`wit/nlipsy.wit`,
the note on `content`), and `http-options.status`, where a number outside RFC 9110's
range falls back to that field's own default rather than taking an exchange down over a
configuration value.
