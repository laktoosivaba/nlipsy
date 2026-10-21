# echo-endpoint

A Rust end-point for the nlipsy engine, as a WebAssembly component: the five interfaces
[`wit/nlipsy.wit`](../../wit/nlipsy.wit) imports, at their smallest.

It is the deployment half of the picture `wit/README.md` (*The design*) draws. The engine
(`crates/nlipsy-component`) holds the protocol — clause 5, clause 6, the composed order,
and ECMA-431 §6 in both directions — and imports *code*: a handler, layers, a token
authority, a transport, a record sink. This component exports exactly those, `wac plug`
joins the two, and what comes out is one artefact `wasmtime serve` answers `curl` from,
with no JavaScript and no host language anywhere.

What it does:

| interface | this deployment |
|---|---|
| `host-endpoint.answer` | echoes the request's text back as a `text` message |
| `host-endpoint.conformity` / `.rejection-policy` / `.http-binding` | the engine's own defaults, spelled out |
| `host-layers.count` | `0` — no layers |
| `host-authority` | owns nothing, has issued nothing, mints nothing |
| `host-transport.send` | `unreachable`: this deployment supplies no transport |
| `host-record-sink.write` | discards |

The four "nothing" answers are the same defaults a Rust deployment gets from
`nlipsy-core`'s trait default methods. They are written out because **WIT has no optional
imports**: a host implements all five interfaces or the component does not instantiate.

## Building and composing it

```text
cargo +nightly-2026-09-15 build -p echo-endpoint --profile component --target wasm32-wasip3
wac plug target/wasm32-wasip3/component/nlipsy_component.wasm \
    --plug target/wasm32-wasip3/component/echo_endpoint.wasm \
    -o target/wasm32-wasip3/component/nlipsy_echo.wasm
wasmtime serve --addr 127.0.0.1:8080 target/wasm32-wasip3/component/nlipsy_echo.wasm
curl -s -X POST http://127.0.0.1:8080/nlip \
     -H 'content-type: application/json' \
     -d '{"format":"text","subformat":"english","content":"What is Ecma?"}'
```

`make check-component` does the first two and runs one request through the result
in-process; `make serve-example` does the last two.

The composed artefact imports only `wasi:*` — it is a complete server — and exports
`wasi:http/handler@0.3.0`, which is what `wasmtime serve` calls.

## What is deliberately not here

Any rule. This component reads clause 5's vocabulary to build a reply and knows nothing
about the echo, the token ceiling, the minting, the order the engine runs its steps in,
the path, the method, the status line or the correlator. That is the claim the whole
design rests on, and an end-point this small is the shortest way to check it: if
answering `curl` needed one line of protocol here, the line would be in the wrong place.

The conversions in `src/convert.rs` are the third copy in this tree, after the guest's
and the conformance host's, and that is what a contract is for: each side writes its own
and they meet at the WIT. A deployment that reached into the engine's crate for them
would be linking the engine rather than composing with it.
