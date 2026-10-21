# Contributing

Thank you for looking. This workspace has a narrow job: model the NLIP message as
ECMA-430 clause 5 states it, hold the obligations clause 6 puts on an end-point, carry
both over the transports the companion standards define — and say so out loud wherever
the standard does not state something clearly.

There is **no CLA**. Open a pull request; that is the whole process.

## The crate map

Nine members, and which one a change belongs in is usually decided by one question:
*does it need a socket, a clock or a task?*

| crate | holds | published |
|---|---|---|
| `crates/nlipsy-protocol/` | ECMA-430 clause 5: what a message *is*, on the JSON wire and the CBOR one. Depends on nothing else here | yes |
| `crates/nlipsy-core/` | ECMA-430 clause 6: what an end-point *owes* a peer, as synchronous functions over values, the `serve` composer that puts them in order, plus the `Endpoint`, `TokenAuthority`, `Middleware` and `Transport` seams | yes |
| `crates/nlipsy-http/` | ECMA-431 §6 as values: the status map, the path, the correlator header, `Retry-After` both ways, the error bodies a binding builds before clause 6 — shared by both ends, with no socket and no clock | yes |
| `crates/nlipsy-http-client/` | ECMA-431 from a client agent's end — one `POST`, over reqwest or over hyper | yes |
| `crates/nlipsy-server/` | ECMA-431 from a server agent's end — one `axum::Router`, with clause 6 built into the only constructor | yes |
| `crates/nlipsy/` | the facade: the five above as modules behind feature flags, and no code | yes |
| `crates/nlipsy-component/` | the `wit-bindgen` guest of `wit/nlipsy.wit`: the same engine as a WebAssembly component, which composes the host's layers and carries ECMA-431 §6 in both directions over `wasi:http`. No rule of its own | no — experimental |
| `crates/conformance/` | `nlipsy-conformance`, the runner for the corpus — against the crates, and through the built component in wasmtime behind feature `component` | no |
| `examples/echo-endpoint/` | a Rust *end-point* component: the five interfaces the engine imports, at their smallest. `wac plug` joins the two into one artefact `wasmtime serve` serves | no — experimental |

The two marked experimental, together with `wit/` and the npm package in `javascript/`,
are the engine as a WebAssembly component; [the experimental section](#the-experimental-part-the-engine-as-a-webassembly-component)
below is about working on them.

If the answer to the question is *yes*, it is not core work: the core has no executor,
no socket, no clock and no file I/O, it compiles for `wasm32-unknown-unknown`, and
every clause-6 rule in it is testable without an executor. That is not an aspiration,
it is what `make check-wasm` enforces and what the JavaScript package depends on.

**A new binding is a new crate, never a feature of an existing one.** ECMA-432
(WebSocket) and ECMA-433 (AMQP) carry the same messages and the same clause 6 over a
different transport, and each brings things the core must not know — CBOR framing and
a text fallback for one, `correlation-id` and `application-properties` for the other.
A binding that lived inside another would make the second depend on the first, and one
that lived inside the core would put a socket in the crate whose whole point is not
having one. It depends on `nlipsy-core` and `nlipsy-protocol`, it gets a feature on the
facade, and it gets a line in the facade README's table. **What the two ends of one
standard must agree on is a crate too** — `nlipsy-http` is ECMA-431's, and ECMA-432's
frame constants would be `nlipsy-ws`'s — because a constant or a status map that lived
in only one of the two ends would be a rule this workspace states twice. `nlipsy-http`
is what ECMA-431 adds on its own — the path, the method, the status map and
`Retry-After` — and the other two would each hold theirs the same way.

## Running the checks

```
make check
```

is `make check-features`, then `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`,
`cargo test --workspace` and `cargo test -p nlipsy-core`. Everything else the crates'
CI runs is in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) — the workflow a
release waits on, and the only one a `v*` tag triggers; the experimental part has one of
its own, described in its section below. One of `ci.yml`'s jobs is worth running
locally before you push, because it fails for a reason `make check` cannot see:

```
make check-wasm
```

which is `cargo check --workspace --all-features --target wasm32-unknown-unknown` less
the two native HTTP bindings and the facade, plus a second line for the facade with the
features whose crates do compile there (`--all-features` on a facade means `full`, and
`full` means the bindings). The two experimental members are left out of it too: each
is a `cdylib` for `wasm32-wasip3`, and that target is where they are checked. It is
separate from `make check` because the target is an extra `rustup target add` a fresh
checkout may not have, and a check that cannot run is a check that gets commented out.
It exists because wasm-cleanliness is a promise, and it breaks quietly, one dependency
reaching for the clock or the filesystem at a time.

Two of the lines inside `make check` are there for reasons worth knowing, because both
are about a build `--all-features` never produces:

- `cargo test --workspace` — `cbor` is optional, so the default build is a *different*
  crate: an item behind `#[cfg(feature = "cbor")]` that unconditional code refers to
  compiles fine under `--all-features` and not at all without them.
- `make check-features` — every feature set `--all-features` cannot stand in for.
  First the facade, one feature at a time: `nlipsy` has no default features and every
  module it has is behind one, so a feature that forgets part of what it enables
  compiles perfectly whenever something else has turned that part on, and fails only
  for the consumer who asked for exactly that feature. Thirteen explicit lines, one per
  manifest somebody might write — three of them pairs, because `nlipsy::core`,
  `nlipsy::http` and `nlipsy::http_client` are each produced by more than one feature
  and a module written as two blocks rather than one `cfg(any(..))` fails only there.
  Then `nlipsy-http-client`'s own two transports: the build with neither, the one an
  Android or an iOS FFI build uses, the one with both, and the hyper suite. Last
  `make check-mobile-graph`, which reads `cargo tree` for `aarch64-linux-android` and
  `aarch64-apple-ios` and fails if `jni`, `rustls-platform-verifier`, `aws-lc`, `cmake`,
  `security-framework`, `openssl` or `native-tls` is in the hyper transport's graph —
  the property that transport exists for, which no amount of Rust can state.

`cargo deny check all` covers licences, advisories and dependency sources; see
[`deny.toml`](deny.toml).

## Minimum supported Rust version

Declared once, as `rust-version` in the root manifest's `[workspace.package]`, and
read back from there by CI — never written out a second time.

While the version is 0.x, **raising the MSRV is a breaking change**, as the crate
README says. It gets a minor bump and a changelog entry of its own. Please do not
raise it as a side effect of something else: if a dependency bump needs a newer
compiler, that is the change, and it should be reviewed as one.

`crates/nlipsy-component/` and `examples/echo-endpoint/` are the two members that
declare a `rust-version` of their own, and they may: `wit-bindgen` 0.62's generated code
is edition 2024 and past 1.85, both are `publish = false`, and nothing published depends
on either — so their floors are not promises to anyone. Every MSRV check excludes them by
name and says why at the exclusion. Do not copy the exception into a published crate.

## The standard, and how it is read

ECMA-430 is short and does not always agree with itself. That is the interesting
part of this crate, so the way disagreements are handled is a rule rather than a
habit:

- **Every reading of the standard is documented in the crate README**, under
  *Readings of the standard this crate takes*, with the clause it comes from and the
  reason the text was read that way. A reading that lives only in the code is a
  surprise waiting for a user.
- **Every reading is covered by a case in `conformance/`.** The corpus is *recorded*
  rather than composed wherever the standard prints something to record: the standard's
  own worked examples, TR/113's figures, bytes from a live endpoint. A fixture built
  with this workspace's own encoder agrees with a systematic misreading exactly as
  readily as with a correct one, which is why a fixture produced by something other
  than this code is the only kind that counts. The corpus is data, and the runner for
  it is the unpublished member `crates/conformance` —
  `cargo test -p nlipsy-conformance --all-features`, which `make check` already reaches
  through `--workspace`. It is a member rather than a test inside `nlipsy-protocol`
  because the cases sit above every package and cannot travel inside a published
  `.crate`; `crates/conformance/src/lib.rs` is the harness, so a second member — an
  ECMA-432 binding, say — can run the same cases over what it transcoded.
- **Code comments cite the clause**, not the conclusion — `§5.1.5`, `Table 1`,
  `TR/113 Figure 9` — so that the next reader can check the claim against the
  document instead of taking the comment's word for it.

Comments here explain *why*. What the code does is visible in the code.

## Where a new case goes

The corpus has **two kinds of case**, and which one a new case is depends on whether it
is about a message or about an exchange.

- A **message case** is bytes in, decode-and-validate out. Everything clause 5 settles
  is one of these: which field is required, how a format value is matched, what a wire
  can and cannot carry, what `validate` reports. Its input is raw *text* (or hex, for a
  CBOR frame), because casing, duplicate keys, whitespace and base64 padding are things
  the corpus has cases about and a parsed value would have thrown all of them away.
- An **exchange case** (`"kind": "exchange"`) is a request, a scripted end-point, and
  the properties clause 6 obliges the finished reply to have. Everything about a
  *pair* of messages is one of these, because no message case can reach it: which
  tokens come back and which are withheld, what is created and when, whether control
  was carried, what was refused. Its `request` and `endpoint.reply` are JSON *objects*,
  because decoding is not what it tests and every byte-level question already has a
  message case.

Then the group, which is about evidence and not about subject:

| directory | for |
|---|---|
| `cases/ecma-430/` | what the standard itself states — Annex A, §5.3 Table 1, clause 6 |
| `cases/ecma-432/` | the WebSocket binding's wire: CBOR frames, the text fallback, its errors |
| `cases/tr-113/` | the explanatory report's sample exchanges, as printed |
| `cases/readings/` | a reading this workspace takes where the standard is silent, ambiguous, or disagrees with itself — and the negative cases that pin it |

Number the file after the last one in its group, say where the input came from
(`source` for a message case, `provenance` for an exchange case), and put the doubt in
`notes`: a case whose expectation is arguable is worth more with the argument attached
than without it. `conformance/README.md` is the schema for both kinds, including the
algorithm an implementation in another language follows — which is the point of the
corpus, so a new expectation belongs in that file before it belongs in a case.
## The Ecma documents are not in this repository

ECMA-430 and its companions are publications of Ecma International. They are not
redistributed here, and they must not be committed — not as PDFs, not as extracted
text, not as quoted paragraphs beyond the short citations already in the source.

They are free to download. From <https://ecma-international.org/publications-and-standards/standards/>,
fetch the ones you need — ECMA-430 (message model), ECMA-431 (HTTP binding),
ECMA-432 (WebSocket binding), ECMA-433 (AMQP binding), ECMA-434, and TR/113, the
technical report with the worked exchanges.

The local convention is to keep them, converted to markdown, in an `ecma-spec-md/`
directory at the repository root, and to exclude that directory **outside** version
control:

```
printf 'ecma-spec-md/\n' >> .git/info/exclude
```

`.git/info/exclude` rather than `.gitignore` on purpose: `.gitignore` is versioned
and would carry a rule about Ecma's documents into a public repository, which is
both noise and an invitation.

## The experimental part: the engine as a WebAssembly component

`crates/nlipsy-component`, `examples/echo-endpoint`, `wit/` and `javascript/` are the
same engine built as a WebAssembly component, the contract it is built from, and an npm
package over it. It is an experiment, and it is kept apart from the crates on purpose:
none of it is published or used in production by anyone yet, nothing published depends
on it, and a bare `cargo build` at the root does not build it (the root manifest's
`default-members` leaves the two crates out; `--workspace` and `-p` reach them). Its CI
is a workflow of its own, [`.github/workflows/component.yml`](.github/workflows/component.yml),
with the `component` and `javascript` jobs: it runs on every push to `master` and every
pull request, weekly, and by hand, and it gates no crate release — a red run there is a
fact about the component or about jco, not about anything a crate ships.

The npm package `nlipsy` lives in [`javascript/`](javascript/): jco's transpile of the
component above plus a thin TypeScript wrapper that loads it and adapts the user's
objects to its imports, and that holds **no rule of the protocol, no composition and no
HTTP code**. `make check-js` is its gate and it is **on** — the whole 125-case corpus
passes through the package in Node and in Bun, as do the glue's own suites. Three seams
jco does not run yet (`wasi:http/handler`, `wasi:http/client`, and a host callback
suspended across a turn of the event loop); their suites are written, are skipped with
the reason printed, and are `make check-js-e2e`, which is red by design.
[`javascript/README.md`](javascript/README.md), *What runs today*, has the measured
detail, and [`wit/README.md`](wit/README.md), *The design*, the reason the wrapper is as
thin as it is.

The same question decides what belongs there as decides what belongs in a crate, read
from the other end: if it is a rule, a composition or a binding, it is Rust in the
component, and if the wrapper seems to need one, the contract is what is missing.

Two targets cover the interface contract and the component built from it, and
neither is part of `make check` for the reason `check-wasm` is not — each needs a tool a
fresh checkout does not have, and a check that cannot run is a check that gets commented
out:

```
make check-wit
make check-component
```

`check-wit` resolves [`wit/nlipsy.wit`](wit/nlipsy.wit), the *interface* contract, the
way `conformance/` is the behaviour one, and checks that every seed it and
[`wit/README.md`](wit/README.md) cite names an item defined where it says — a citation
is a path and an item, never a line, and that README's §2 is the exact rule; it needs
`cargo install wasm-tools --locked`.
`check-component` needs that too, plus the `wasm32-wasip3` target — and a **nightly** to
install it on, because `rust-std` for that target ships on stable only at Rust 1.100
(2026-11-12). The `Makefile` pins the date in one variable:

```
rustup toolchain install nightly-2026-09-15
rustup target add wasm32-wasip3 --toolchain nightly-2026-09-15
make check-component                              # uses that nightly
make check-component COMPONENT_TOOLCHAIN=nightly  # or yours
```

On 2026-11-12 `COMPONENT_TOOLCHAIN` becomes `stable` and the two lines above go.
`nlipsy-component`'s own `rust-version` — and `echo-endpoint`'s — stays at what the
*crate* needs, because each is also an `rlib` whose native tests `make check` runs on
stable; the target's floor belongs to the toolchain that builds the artefact and not to
the source.

`check-component` needs a **fifth** tool as well — `cargo install wac-cli --locked` —
because it also composes. It does six things: lints the corpus runner's `component` mode;
builds `crates/nlipsy-component` for `wasm32-wasip3` with the root manifest's `component`
profile; validates the artefact and reads its world back with `wasm-tools` — which is
also what shows its imports are WASI **0.3** and nothing else, `wasi:cli@0.3.0`,
`wasi:clocks@0.3.0`, `wasi:http@0.3.0` and `wasi:random@0.3.0`, with no `wasi:io`,
because 0.3 carries streams natively; builds `examples/echo-endpoint` the same way and
`wac plug`s the two into one artefact that imports only `wasi:*`; and runs the whole of
`conformance/` through the engine in wasmtime, linked with `wasmtime_wasi::p3` and
`wasmtime_wasi_http::p3`: 125 cases, the asynchronous properties — the composed layer
order, a synchronous export re-entered while an asynchronous one is suspended on a host
import, two exchanges in flight keeping their own context, the replaying client — the
HTTP-level tests through `wasi:http/handler`, the client half against a loopback socket,
and two requests through the *composed* artefact, which has no host of its own at all.
The first run compiles a WebAssembly runtime and takes a few minutes; after that it is
seconds. `NLIPSY_COMPONENT=<path>` and `NLIPSY_COMPOSED=<path>` run against artefacts
built somewhere else.

`make serve-example` puts the composed artefact on a socket with `wasmtime serve`, for
looking at by hand:

```
make serve-example
curl -s -X POST http://127.0.0.1:8080/nlip \
     -H 'content-type: application/json' \
     -d '{"format":"text","subformat":"english","content":"What is Ecma?"}'
```

One thing to know before touching either: `nlipsy-conformance`'s `component` feature is
deliberately kept out of every `--all-features` line, in the `Makefile` and in CI, and
each exclusion says so. The mode loads a built artefact, so a run with the feature on and
no artefact present fails for want of a file rather than for want of conformance.

## Licence

Licensed under either of Apache License, Version 2.0
([LICENSE-APACHE](LICENSE-APACHE)) or the MIT licence ([LICENSE-MIT](LICENSE-MIT)),
at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 licence, shall be
dual-licensed as above, without any additional terms or conditions.
