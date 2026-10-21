# nlipsy

[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)

NLIP — the Natural Language Interaction Protocol — is the Ecma International standard
(ECMA-430) for agents that exchange messages written in natural language. `nlipsy` is
what a Rust author building such an agent depends on: the message model, the
obligations clause 6 puts on every end-point, and both ends of the HTTP binding — a
client agent's `POST` to a peer, and a server agent's `axum::Router` with clause 6
underneath it by construction. These crates are what a working NLIP server deployment
runs today.

Five crates implement the standard — the message model (clause 5), the runtime core
(clause 6 and the sequence a binding drives around it), ECMA-431 §6 as values, and
ECMA-431 from both ends: a client binding that sends one message and a server binding
that answers one — and a sixth, the facade, puts them behind one dependency. It is a
workspace rather than a lone package for the reason the bindings demonstrate: ECMA-431
(HTTP), ECMA-432 (WebSocket) and ECMA-433 (AMQP) each carry the same messages over a
different transport, and a binding belongs *beside* the message model rather than
inside it — so that a second binding never has to depend on the first, and so that a
consumer who only parses messages never compiles a socket. The workspace is the shape
that admits them without moving anything.

## Quick start

```toml
[dependencies]
nlipsy = { version = "0.1", features = ["client"] }   # or "server", or both
```

A client agent (ECMA-430 §6.1, ECMA-431 §6) — a message, the per-exchange context, and
the transport that carries them:

```rust
use nlipsy::core::{Client, Context};
use nlipsy::protocol::{Message, Submessage};

// ECMA-430 §6.1: a client agent's initial request is a message with a first
// submessage. §6.2's conversation token rides beside it.
let request = Message::text("What is Ecma?")
    .with_submessage(Submessage::token("conversation", "c-1"));

// What the runtime knows about this exchange and the message does not. The
// correlator is ECMA-431's `x-nlip-correlator`, sent only when one is set.
let cx = Context::new().with_correlator("turn-1");

let client = Client::new(
    nlipsy::http_client::HttpTransport::new("https://agent.example.com"),
);

// One `await` over the transport: encode, exchange, decode.
async fn ask(client: &Client, request: &Message, cx: &Context) -> Option<String> {
    client.send(request, cx).await.ok()?.text()
}
let _ = ask(&client, &request, &cx);
```

A server agent (`features = ["server"]`) — an endpoint, the authority that answers
§6.2's exception, and a router:

```rust
use nlipsy::core::{Context, Endpoint, EndpointError, TokenAuthority};
use nlipsy::protocol::Message;
use nlipsy::server::{Server, ServerConfig};

struct Echo;

impl Endpoint for Echo {
    fn name(&self) -> &str {
        "echo"
    }

    async fn handle(&self, request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
        Ok(Message::text(request.content_as_str().unwrap_or_default()))
    }
}

// §6.2's exception, answered by the deployment: this endpoint creates no
// tokens, so it created none of the peer's either and every one comes back.
struct NoTokens;

impl TokenAuthority for NoTokens {
    fn is_own(&self, _token: &str, _cx: &Context) -> bool {
        false
    }
}

// The only constructor there is, so clause 6 is underneath the router by
// construction rather than by remembering to put it there.
let server = Server::new(Echo, NoTokens, ServerConfig::new());

// `POST /nlip` and `POST /nlip/`, and nothing else. A deployment merges its
// own routes onto this and hands the result to `axum::serve`.
let app: nlipsy::server::axum::Router = server.router();
let _ = app;
```

Both are the facade's own doctests, from `crates/nlipsy/src/lib.rs`, which
`cargo test --workspace --all-features` compiles; the client one there also builds the
same `Client` over `HyperTransport`, the transport an Android or an iOS build over FFI
asks for. [`crates/nlipsy/README.md`](crates/nlipsy/README.md) is the feature table, and
the two bindings' READMEs are where a deployment's choices are —
[`nlipsy-http-client`](crates/nlipsy-http-client/README.md) for the two transports, the
connect budget and record/replay,
[`nlipsy-server`](crates/nlipsy-server/README.md) for layers, the body limit, the status
map and a trusted proxy's principal header.

## Layout

Nine members. Six are published to crates.io and three are not.

| member | is | published |
|---|---|---|
| [`crates/nlipsy-protocol/`](crates/nlipsy-protocol/) | the NLIP message model — ECMA-430 clause 5, on the JSON wire and the CBOR one | yes |
| [`crates/nlipsy-core/`](crates/nlipsy-core/) | the runtime core — ECMA-430 clause 6 as synchronous steps over values, the `Endpoint`, `TokenAuthority`, `Middleware` and `Transport` seams, and the `serve` composer a binding drives | yes |
| [`crates/nlipsy-http/`](crates/nlipsy-http/) | ECMA-431 §6 as values — the status map, the path, the correlator header, `Retry-After` in both directions, the error bodies a binding builds itself | yes |
| [`crates/nlipsy-http-client/`](crates/nlipsy-http-client/) | the ECMA-431 client binding — one `POST /nlip`, over reqwest or over hyper, with the file-backed recorder | yes |
| [`crates/nlipsy-server/`](crates/nlipsy-server/) | the ECMA-431 server binding — one `axum::Router` with clause 6 underneath it | yes |
| [`crates/nlipsy/`](crates/nlipsy/) | the facade: one dependency, the five above as modules behind feature flags | yes |
| [`crates/nlipsy-component/`](crates/nlipsy-component/) | the `wit-bindgen` guest of [`wit/nlipsy.wit`](wit/nlipsy.wit): the same engine as a WebAssembly component, which composes the host's layers and carries ECMA-431 §6 in both directions over `wasi:http` | **no** — experimental |
| [`crates/conformance/`](crates/conformance/) | `nlipsy-conformance`, the runner for the corpus below — against the crates, and (feature `component`, the experimental part) through the built component in wasmtime | **no** |
| [`examples/echo-endpoint/`](examples/echo-endpoint/) | a Rust *end-point* component: the five interfaces the engine imports, at their smallest. Composed with the engine by `wac`, it is one `.wasm` that `wasmtime serve` answers `curl` from | **no** — experimental |

and, outside the members:

```
Cargo.toml                 the workspace: shared version, dependency versions, lints
Cargo.lock                 one lockfile for every member
conformance/               the corpus: recorded messages and scripted exchanges, and
                           what every implementation must produce for each
javascript/                experimental: the npm package `nlipsy`, a loader and five
                           adapters over the component, and no rule of the protocol
wit/                       experimental: the interface contract — what the engine
                           offers a host in any language, and what it asks of one
wit/deps/                  WASI 0.3.0's own WIT, vendored from the wasip3 crate, so
                           that the world can reach wasi:http and wasi:random
```

The two members marked experimental, together with `javascript/` and `wit/`, are the
engine as a WebAssembly component. The
[experimental section](#experimental-the-engine-as-a-webassembly-component) below is
about them, and nothing published depends on any of it.

`nlipsy-conformance` is a member so that `cargo test --workspace` runs it, and
`publish = false` because a test runner for one repository's corpus is nobody's
dependency — and because the corpus it runs sits above every package and could not
travel inside a `.crate` even if it were published.

A bare `cargo build` or `cargo test` at the root covers the six published crates and
`nlipsy-conformance` — the root manifest's `default-members`. `--workspace` reaches the
two experimental members as well, on the host; their own target is
`make check-component`'s.

## How the pieces fit

`nlipsy-protocol` is **clause 5**: what a message *is*. A first submessage, an ordered
list of further ones, six formats, two wires, and the rules `validate` reports findings
against. It knows nothing about exchanges, and it depends on nothing in this workspace.

`nlipsy-core` is **clause 6** over it: what an end-point *owes* a peer — return every
token submessage it was sent except the ones it created itself, keep confirming the one
it issued, answer a control request with a control reply, refuse to emit a reply that
breaks clause 5. Those are two synchronous functions over values, one before the
handler and one after it, plus the seams a runtime plugs into: `Endpoint`,
`TokenAuthority`, `Middleware`, `Transport`. It also holds the *order* those steps run
in — `serve`, the composer a binding drives: decode, check, the layers with `finish` at
the bottom of them, `close` over whatever came back, and the failure rules. There is no
executor in it, no socket, no clock and no file I/O — that boundary is the whole design,
and `make check-wasm` is what enforces it.

`nlipsy-http` is **ECMA-431 §6 as values**, and it has no socket either. §6 fixes the
path and the method and nothing else, so which status a refusal carries, what
`Retry-After` means in both directions, and what body a `405` or a `413` has are
choices — and a choice made twice is a choice made differently. Both ends of the HTTP
binding read them from here, and so will a `fetch` handler in another language over the
same engine.

The **bindings** are the crates that do have a socket, and each is one direction of one
transport. `nlipsy-http-client` is ECMA-431 from a client agent's end; `nlipsy-server`
is ECMA-431 from a server agent's, with clause 6 built into the only constructor there
is. Neither depends on the other — both depend on `nlipsy-http`, which is the shape
that lets them agree without either one owning the clause. ECMA-432 and ECMA-433 join
them as further crates, not as features of these.

`nlipsy` is the **facade** over all five: one line in a consumer's manifest, the crates
behind it as modules named by feature. It contains no NLIP code.

The order of dependence is the order of the table — protocol, core, bindings, facade
— and it is also the publishing order, for the same reason.

## The facade

`crates/nlipsy/` contains no NLIP code. It re-exports each `nlipsy-*` crate as a
module behind a feature flag — `nlipsy::protocol` is `nlipsy-protocol` — the way
`jsonrpsee` re-exports `jsonrpsee_types` as `jsonrpsee::types`. It has no default
features, so depending on it costs nothing until a feature is named, and depending on
`nlipsy-protocol` directly stays equally supported: the facade is for the consumer's
manifest, not for their imports.

The bare name belongs to the facade rather than to the message model on purpose.
`nlipsy` is what someone types when they want "NLIP in Rust", and what they get under
that name should be the whole protocol as it grows — model plus bindings, each opted
into — rather than clause 5 alone. Had the model taken the name, adding HTTP would
have meant either swelling the model crate with a transport or telling every consumer
to switch dependencies; the split names cost one extra line in a manifest once, and
nothing after that.

Read [`crates/nlipsy-protocol/README.md`](crates/nlipsy-protocol/README.md) for the
message model: what it models, what it deliberately leaves out, and the readings of
the standard it takes where ECMA-430 does not agree with itself. Read
[`crates/nlipsy-core/README.md`](crates/nlipsy-core/README.md) for the runtime core:
clause 6 as three synchronous steps, the `serve` composer that puts them in order, the
`Endpoint`, `Transport` and `Middleware` seams, and the boundary that keeps a runtime
out of it — one test decides what belongs there, and it
is the same `wasm32` question `make check-wasm` asks.
[`crates/nlipsy-http/README.md`](crates/nlipsy-http/README.md) is the clause both HTTP
bindings read: why a status code is a choice and the type says so, why
`parse_retry_after` takes `now` as an argument rather than reading a clock, and why the
two constants that used to be declared twice on purpose are declared once now.

[`crates/nlipsy-http-client/README.md`](crates/nlipsy-http-client/README.md) is the
first crate on the other side of that boundary: the ECMA-431 client binding, which is
one `POST` to `https://<server_name>:port/nlip` and the file-backed half of the core's
recorder. It is where the runtime concerns the core refuses live — a socket, a connect
budget, `Retry-After`, a path on disk — and it is one of the two members
`make check-wasm` excludes, because reqwest on `wasm32` is the fetch backend and a
*native* HTTP binding was never what that promise covered. Its README's "Two
transports" is the one section to read before depending on it: the same exchange is
implemented twice, over reqwest and over hyper, because reqwest's TLS depends on
rustls-platform-verifier unconditionally and that means `jni` plus a Kotlin companion
library on Android and a cmake-built BoringSSL everywhere — which an app over FFI cannot
carry and no feature can remove. The hyper transport is the same `POST` with `cc`,
`ring` and `webpki-roots` under it, and `make check-mobile-graph` is what keeps it that
way. Its README also names what
the merge cost: the client binding used to declare `/nlip` and `X-NLIP-Correlator`
independently of the server, in another repository, so drift between the two failed
loudly; in one workspace that witness is gone, and the corpus below and the recorded
fixture are what replace it.

[`crates/nlipsy-server/README.md`](crates/nlipsy-server/README.md) is the same clause
from the other end: an `axum::Router` for `POST /nlip`, with clause 6 underneath it and
the deployment's own work — throttling, sessions, a deadline — stacked above it as
message-level layers. Its one structural rule is that there is no way in that skips the
obligations: `Server::new` takes an `Endpoint` and a `TokenAuthority` and builds
`Conformity` itself, so the §6.2 echo and the §6.3 carry cannot be displaced by a
layer, which is jsonrpsee's arrangement and is there because the echo is the obligation
a reply built from scratch silently drops. The other thing it settles is the one
rejection the reference server answered out of protocol: the body limit is enforced
while the body is read, so an oversized request gets an NLIP error message rather than
a framework's plain text.

## Scope

What this workspace implements, by standard, and what it deliberately does not:

- **ECMA-430 clause 5**, the message model: whole, on both wires. Clause 5's JSON is
  what every crate speaks; ECMA-432 §7.1's CBOR spelling of the same message is
  `nlipsy-protocol`'s `cbor` feature, because it is an encoding of the message rather
  than a way of moving it.
- **ECMA-430 clause 6**, the mandatory exchanges: whole — the token echo of §6.2, the
  conversation and authentication tokens of §6.2.1 and §6.2.2, the control exchange of
  §6.3 and the out-of-band constructors of §6.4 — as `nlipsy-core`, with no executor,
  socket or clock in it.
- **ECMA-431 §6**, the HTTP binding: both ends, `nlipsy-http-client` and
  `nlipsy-server`, over the values `nlipsy-http` holds for them; the experimental
  component carries the same binding through `wasi:http`.
- **ECMA-432 beyond §7.1: no WebSocket binding.** The frames, the §11 fallback
  end-point and the heartbeat are a binding's, and a binding is a crate of its own.
  §10's optional "custom field" has no place in the model, which drops every field
  clause 5 does not define; a session id travels in `MessageType`, as §10 also allows.
- **ECMA-433: no AMQP binding**, and no AMQP-type encoding of the message (§6.1.7). By
  the test that admits CBOR that encoding would belong in the model too; it is deferred
  because the text leaves its shape underspecified.
- **ECMA-434**, the security profiles: deployment requirements, outside every crate.
  The one place the engine follows it is §7.3.3 — an end-point's internal failures
  reach the peer as an opaque message.
- **TR/113** is explanatory, not normative; its worked exchanges are the corpus's
  `tr-113/` group.

Where ECMA-430 does not agree with itself, the reading this workspace takes is written
in the crate's README under its *Readings* section and pinned by a case in
`conformance/`; [CONTRIBUTING.md](CONTRIBUTING.md) is the rule.

## The corpus

[`conformance/`](conformance/) is a corpus rather than a test suite, and it has two
kinds of case. A **message case** is bytes in and decode-and-validate out — JSON or
CBOR, each recording where its bytes came from and what a conforming implementation
must produce for them: that is clause 5. An **exchange case** is clause 6, which no
message case can reach, because clause 6 is about a *pair* of messages and about what
an end-point remembers having created: a request, a scripted end-point, the reply its
handler produced, and the properties the finished reply must have — which tokens came
back, whether control was carried, what was refused. 125 cases, 101 and 24.

Nothing in it is Rust, on purpose — the JavaScript package of the experimental section
runs all of it with a runner of its own, and a corpus that only one language can run is
an assertion about one language. The exchange cases are shaped for that: clause 6 is
two synchronous steps over values, so running one needs no server, no task and no
clock. This workspace is the corpus's first
consumer — `crates/conformance/` runs the message cases against `nlipsy-protocol` and
the exchange cases against `nlipsy-core` — so `make check` runs the whole of it.

## Checks

```
make check
```

runs, over the whole workspace:

- `make check-features` first — see below
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo test --workspace` — the same tests with no optional feature on, which is how
  every consumer that does not ask for `cbor` builds the crate
- `cargo test -p nlipsy-core` — the core with *no* features, which `--workspace` stopped
  reaching: `nlipsy-http-client`'s dev-dependency asks for `client` and `testing`, and
  feature unification turns those on for the whole run, so the plain build — what
  `nlipsy-core = "0.1"` gives a consumer who names nothing else — needs a line of its own

```
make check-features
```

is every feature set `--all-features` cannot stand in for. `--all-features` proves
nothing about a crate whose every module is behind a feature and whose features nest: a
`http-client` that pulled the binding without the core's `client` half compiles
perfectly when something else has turned that on, and fails for the one consumer who
asked for only that. Each line of the target is a manifest somebody might actually
write — thirteen for the facade, spelled out rather than generated, so a reader can
check the list against the feature table. Three are pairs rather than single features,
and each is there because a module is produced by *two* of them:
`client-core,server-core` for `nlipsy::core`, `http-client,server` for `nlipsy::http`,
and `http-client,http-client-hyper` for `nlipsy::http_client`, which has a transport
feature each for reqwest and hyper. A module written as two blocks instead of one
`cfg(any(..))` fails on exactly those lines and nowhere else.

Four more lines are the client binding's own feature sets — with neither transport,
with hyper alone, with both, and the hyper suite run — and the last line is
`make check-mobile-graph`, which is not a build at all: it reads
`cargo tree -p nlipsy-http-client --no-default-features --features hyper` for
`aarch64-linux-android` and `aarch64-apple-ios` and fails if `jni`,
`rustls-platform-verifier`, `aws-lc`, `cmake`, `security-framework`, `openssl` or
`native-tls` has appeared in either. That property is what the crate's second transport
*is*, and nothing in the source can state it: one transitive default re-enabled three
crates down would bring the whole of it back and compile perfectly here. `cargo tree
--target` needs neither target installed nor an NDK nor Xcode, so it runs wherever cargo
does; compiling for those targets is the FFI crate's job on a machine that has both.

```
make check-wasm
```

is separate, and checks the workspace for `wasm32-unknown-unknown`: the crates have to
stay free of anything that target does not have — a socket, a clock, a thread, a file —
and that is the property that keeps the core free of I/O. It is the test that decides
what belongs in the core at all, and why `nlipsy-core`'s traits are bounded by a
`MaybeSend` marker rather than by `Send` — a JavaScript callback is neither. It runs on
stable and is cheap; it is not part of `make check` only because the target is an extra
`rustup target add` that a fresh checkout may not have, and a check that cannot run is
a check that gets commented out.

It covers `nlipsy-http` — ECMA-431 §6 as values has no socket and no clock in it, which
is half the reason it is a crate — and excludes `nlipsy-http-client` and `nlipsy-server`,
which is the promise stated precisely rather than a hole in it: the guarantee is for the crates a JavaScript package
compiles, and a *native* HTTP binding — either direction — is not one of them. reqwest
on that target is a different client, the fetch backend, with no `connect_timeout` and
no `is_connect`, which are the two things the client binding's contract is written in
terms of; and the server binding is axum over hyper over tokio, an accept loop and a
socket, which a browser has nothing to point at. A JavaScript package brings its own
`fetch` and its own HTTP framework and reaches clause 6 through `nlipsy-core`, which is
why the layer seam and the dispatcher live there rather than in the binding.

The facade is excluded from that line too, and checked on a line of its own — because
`--all-features` on a facade means `full`, and `full` means both of the crates above.
The second line names `client-core`, `server-core` and `cbor`: every facade feature
whose crate compiles for this target, which is also what a consumer reaching the core
from a browser would actually write. It cannot reach `nlipsy::http`, whose two features
are the two bindings, so the crate underneath that module is covered by the
`--workspace` line instead. The promise is checked rather than skipped. The two
experimental members are excluded as well, for a reason of target rather than of
promise: each is a `cdylib` for `wasm32-wasip3`, and `make check-component` checks it
there.

CI is two workflows. [`ci.yml`](.github/workflows/ci.yml) runs everything above — fmt,
clippy, the tests on Linux, macOS and Windows, the feature sets, the MSRV build, rustdoc
as docs.rs builds it, `wasm32-unknown-unknown` — and `cargo-deny`, `cargo-machete`,
`cargo-semver-checks` and `typos` besides. It is the status a release waits on, and the
only workflow a `v*` tag triggers.
[`component.yml`](.github/workflows/component.yml) is the experimental part's, and the
section below says what it runs.

## Experimental: the engine as a WebAssembly component

[![Component](https://github.com/laktoosivaba/nlipsy/actions/workflows/component.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/component.yml)

The repository also builds the same engine as a WebAssembly component, and carries the
interface contract it is built from and an npm package over it. It is an experiment,
kept here and kept apart: nobody has used the component or the package in production
yet, neither is published, and the six crates above neither depend on it nor wait for
it — they carry no wasm dependency at all.

It is the same engine and not a second one. `crates/nlipsy-component` is the
`wit-bindgen` guest of [`wit/nlipsy.wit`](wit/nlipsy.wit), and it contains no rule of its
own — every export is `nlipsy-protocol`'s, `nlipsy-core`'s or `nlipsy-http`'s, re-spelled
in the contract's vocabulary, and [`wit/README.md`](wit/README.md) §3 is the table of
which, each row naming the Rust item it is lifted from. What it *does* contain is the
composition: a host supplies a handler, layers, a token authority, a transport and a
record sink, and everything else — clause 5, clause 6, the order they run in, and
ECMA-431 §6's path, method, body limit, status line, `Retry-After` and correlator — is
inside. It is `publish = false` because what a consumer would install is the npm package
built from the artefact, not the crate.

`make check-component` builds it for `wasm32-wasip3`, runs the whole corpus through it
in wasmtime, drives its `wasi:http/handler` export, and composes it with
[`examples/echo-endpoint`](examples/echo-endpoint/) — a Rust *end-point* component, the
five interfaces the engine imports at their smallest — into an artefact a runtime serves
on its own:

```text
make check-wit
make check-component
make serve-example
curl -s -X POST http://127.0.0.1:8080/nlip \
     -H 'content-type: application/json' \
     -d '{"format":"text","subformat":"english","content":"What is Ecma?"}'
```

`make check-wit` resolves the contract and checks that every seed it cites still names an
item defined where it says. These targets need what a fresh checkout does not have —
`wasm-tools`, `wac`, the `wasmtime` CLI for `serve-example`, and a **nightly** with the
`wasm32-wasip3` target, because `rust-std` for that target ships on stable only at Rust
1.100, on 2026-11-12. The `Makefile` pins the nightly in `COMPONENT_TOOLCHAIN`, and
[CONTRIBUTING.md](CONTRIBUTING.md) has the commands.

The npm package `nlipsy` is [`javascript/`](javascript/): jco's transpile of the same
component and a thin TypeScript wrapper that loads it and adapts the user's own objects
to its imports, holding no rule, no composition and no HTTP code. `make check-js` builds
and tests it, all 125 conformance cases through the package included. Three seams jco
cannot run yet — `wasi:http/handler`, `wasi:http/client`, and a host callback that
suspends across a turn of the event loop — are `make check-js-e2e`, which is red by
design: measured on 2026-10-21 against jco 1.39.0 and preview3-shim 0.11.0, while the
same paths run under wasmtime ([`javascript/README.md`](javascript/README.md), *What runs
today*). The package is not published until they run.

CI for all of it is a workflow of its own,
[`component.yml`](.github/workflows/component.yml): `make check-wit`,
`make check-component` and `make check-js`, on every push to `master`, on every pull
request, weekly, and by hand. A red run there is a fact about the component or about
jco, and it is never the status a crate release waits on.

## Taking part

- [CONTRIBUTING.md](CONTRIBUTING.md) — the checks, the MSRV policy, how a reading of
  the standard gets documented and covered by a case, and why the Ecma documents are
  not in this repository.
- [CHANGELOG.md](CHANGELOG.md) — what changed, per release.
- [SECURITY.md](SECURITY.md) — reporting a vulnerability privately.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 licence, shall be
dual-licensed as above, without any additional terms or conditions.

ECMA-430, ECMA-431, ECMA-432, ECMA-433, ECMA-434 and TR/113 are publications of Ecma
International and are not redistributed here.
