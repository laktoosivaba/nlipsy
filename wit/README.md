# The interface contract

`nlipsy.wit` is the language-neutral interface of the nlipsy engine: the types and the
functions a host in any language gets when it loads the component built from this
workspace. It is one of two contracts, and the other is `conformance/cases/`: the corpus
is the truth about **behaviour** — what an implementation must produce for given bytes —
and this file is the truth about the **interface** — what an implementation offers and
what it asks of its host. A second implementation, or a second host language, is
measured against both.

It is a repository file and is published in no package. `crates/nlipsy-component` is
built from it, `jco` turns that component into ESM and `.d.ts` for the npm package in
`javascript/`, and a Python or Go host loading the same component through wasmtime reads
this file to know what it is holding.

## The design

**The protocol lives in the component.** It composes the host's layers over the host's
handler, it carries ECMA-431 §6 itself through `wasi:http/handler@0.3.0`, and it is the
client too through `wasi:http/client@0.3.0`: the path, the method, the body limit, the
status line, `Retry-After`, the correlator header and the content type are all decided
inside, with the values of `nlipsy-http` and the composition of `nlipsy-core::serve`.
Any host that speaks WASI 0.3 runs it — wasmtime, `wasmtime serve`, jco.

**The host supplies code, not protocol.** Five imported interfaces, all of plain
functions: the end-point (its configuration and its `answer`), the layers, the token
authority, an alternative transport, a record sink. What a layer is allowed to do is
defined by the guest, which runs the stack; what an authority answers is the host's. If
a host ever seems to need a rule, a status code, a header name or a decision about which
layer runs first, the contract is what is missing.

**No resource crosses the boundary in either direction.** `context` is a record by value
carrying an `exchange` number the host keys its own state on, so nothing is borrowed,
nothing is lent and nothing leaks; the guest holds no state between calls and builds its
`Conformity` per call. Resources are also the part of the canonical ABI that host tooling
gets wrong most often — constructors, borrow capture tables — and the part that needs
`unsafe` in a guest to carry a handle across an `await`; a contract with none has nothing
there to get wrong, by construction rather than by fix.

## 1. The rule: no export without a seed

Every item in `nlipsy.wit` cites, in its doc comment, the clause it implements and the
**seed** it is lifted from — an item in this workspace, named by the file that defines
it and its Rust path inside that file: `crates/nlipsy-core/src/serve.rs::serve`, or
`crates/nlipsy-core/src/conformity.rs::Conformity::check` for a member. Never by
line: a line number drifts with every edit above it, and an item moves only when
somebody moves it. An item with no seed may not be added.

The rule is not bookkeeping. The engine's claim is that there is one implementation of
each rule and that a host in a second language contains none of its own. An export with
no seed is a rule that exists only at the boundary: nothing in Rust tests it, the corpus
cannot reach it, and the next binding re-derives it. §3 below is the seed table, and it
is complete — if an export is missing from it, the export is the thing that is wrong.

**One item has no Rust seed, and says so.** `types.cors`, and `http-options.cors` with
it, is the binding's own: `nlipsy-server` serves no browser and has no CORS, ECMA-431 §6
names nothing of the kind, and the rule the guest applies is its own — no cross-origin
header at all unless the request carried an `Origin` that matches, because a `*` beside
`Access-Control-Allow-Credentials: true` is a pair no browser accepts. The `405`'s
`Allow: POST` header is the same shape of exception, and §3 records it the same way: a
value the standard leaves to a binding, written down where a reader meets it rather than
given a constant in Rust that nothing but the boundary would read.

The converse is not a rule. Plenty of `nlipsy-core` is deliberately *not* exported:
`Dispatcher` (a host that wants the control/data split reads `protocol.is-control` and
routes inside its own `answer`), `Recorder` and `Replay` as types (the `client` interface
is four functions over them), and `Conformity::close` with `echo_into` under it. The last
is deliberate: at a boundary with no `&mut`, `close` would become `close(request, reply,
cx) -> message`, which reads like "the echo" and is not — a caller who ran it *instead
of* `finish` would skip clause 5's outbound sanitiser and §6.2.1's minting and never find
out — and the obligation it discharges for a binding's own refusal is reachable through
`engine.error-reply`, which applies it with the authority the host does not hold. What is
exported is what a host needs in order to be an end-point or a client without writing a
rule down a second time.

Doc comments are part of the deliverable, not decoration. wit-bindgen copies them into
the generated Rust; jco does **not** copy them into its `.d.ts` (§5, measured), so this
file is where a *Rust* caller's editor gets its documentation and where the npm
package's `types.ts` restates it from. There is nowhere else for it to come from: a
wrapper's own types are a flattening of these, and a reader who wants to know why
`host-endpoint.answer` returns a `message` rather than a `finished` has only the comment
on `answer` — or the sentence `types.ts` copied out of it.

## 2. How the contract is validated

Five checks, in increasing cost, all of which passed for the file as committed.

**The file parses and the world resolves.**

```sh
wasm-tools component wit wit/
```

It prints the resolved world — every `use` followed, every type reachable — or fails.
It is the `check-wit` target in the `Makefile`:

```makefile
check-wit:
	wasm-tools component wit wit/ > /dev/null
	node wit/check-seeds.mjs
```

It now also reads `wit/deps/`, which is the WASI 0.3.0 WIT this world needs, vendored
from the `wasip3` crate: see `deps/README.md` for the version, the licence and why all
six files are there when the world reaches two of them.

**The seeds still point where they say.** `check-seeds.mjs` is the second line of that
target and it enforces §1's rule rather than restating it. A citation is one backticked
token — a path from the repository root to a `.rs` file, `::`, and one or two Rust path
segments — and it must name an item that file defines:

- **one segment** is a free item: `crates/nlipsy-core/src/serve.rs::serve` needs a
  `fn`, `struct`, `enum`, `trait`, `type`, `const`, `static`, `mod`, `union` or
  `macro_rules!` called `serve` that is not inside an `impl` or a `trait` — a method
  is never cited bare;
- **two segments** is a member of the first: a `fn`, `const` or `type` whose nearest
  enclosing item is an `impl` for that type or the `trait` of that name — which is how
  `crates/nlipsy-http/src/status.rs::StatusMap::default` is found, inside
  `impl Default for StatusMap` — or a field of the struct, or a variant of the enum,
  one level inside its body, as
  `crates/nlipsy-server/src/server.rs::ServerConfig::principal_header` is.

Every token of that shape in `nlipsy.wit` and anywhere in this file is checked, and a
backticked file name followed by a colon and a line number fails outright, so the
retired form cannot creep back in. The source is read as rustfmt leaves it, which
`make check` enforces: an item's body closes at the indentation it opened at, so nesting
is read from indentation and no Rust is parsed, and a `cfg_client! { … }` invocation is
read through rather than taken for an item. A failure names where the item *is* defined
in that file, when it is.

It reads `nlipsy.wit` and this file and nothing else, which is also what keeps `deps/`
out of its way: those are someone else's interfaces and have no seed here.

It exists because a citation by line drifts the moment the file above it grows — and
drifts quietly, because the line it lands on usually still mentions something nearby —
and a table that can be wrong without anything noticing is a convention rather than a
rule. Node and no dependency, so it runs wherever `make check-wit` does; every citation,
in well under a second.

It is **not** part of `check`, for the reason `check-wasm` is not: it needs a tool a
fresh checkout does not have (`cargo install wasm-tools --locked`), and a check that
cannot run is a check that gets commented out. The `Makefile` says so at the target. CI
runs it in the `component` job of `.github/workflows/component.yml`, the workflow kept
apart from the one a release waits on, which installs `wasm-tools` for
`make check-component` anyway and runs this first, because a failure here explains every
failure below it.

**The file compiles.** Parsing is not enough: a keyword used as a name, or a method
that collides with an accessor wit-bindgen generates, parses cleanly and fails only when
the generated Rust is compiled. So a throwaway stub crate — `cdylib`, `wit-bindgen` as its only dependency —
is built for `wasm32-wasip3` outside the repository, and `wasm-tools component wit` is
run on the artefact to see the world come back:

```rust
wit_bindgen::generate!({
    path: "…/wit",
    world: "nlipsy",
    generate_all,
    stubs,
});
```

`generate_all` is not optional: a world that reaches `wasi:http` pulls
`wasi:clocks/types@0.3.0` in transitively, and without it wit-bindgen stops with
`missing one of: * generate_all option * with: { … }`, because it assumes WASI packages
come from a crate. The alternative is a `with:` map onto `wasip3::http`, which would
make the guest depend on a crate for bindings it can generate itself.

The artefact is 177 606 bytes of nothing but bindings. It validates
(`wasm-tools validate --features all`), and its world is:

```wit
world root {
  import nlipsy:engine/types@0.1.0;
  import wasi:clocks/types@0.3.0;
  import wasi:http/types@0.3.0;
  …the thirteen wasi:cli / wasi:clocks interfaces std links…
  export nlipsy:engine/protocol@0.1.0;
  export nlipsy:engine/engine@0.1.0;
  export nlipsy:engine/client@0.1.0;
  export wasi:http/handler@0.3.0;
}
```

Read it precisely. All four exports come back, `async func` intact on `serve`,
`serve-message` and the three asynchronous `client` functions, and `wasi:http/handler`
among them. The five `host-*` interfaces are **absent**, and so are
`wasi:http/client@0.3.0` and `wasi:random/random@0.3.0`: a stub calls none of them, and
`wasm-tools component wit` records only what a guest links. An import list is therefore
not a contract, and a host generated from an *artefact* rather than from this file will
be asked for less than a host generated from this file — which is why the npm package's
types are generated from here.

**The asynchronous imports are proved by the real guest.** `crates/nlipsy-component` is
that guest, and `wasm-tools component wit` on its artefact
(`target/wasm32-wasip3/component/nlipsy_component.wasm`, what `make check-component`
builds) reads the five `host-*` interfaces back with their functions, `async func` and
all. The corpus's `component` mode is the behavioural half of the same proof: 125 cases
of `conformance/` answered by that artefact through wasmtime, with the five interfaces
implemented in Rust — `crates/conformance/src/component.rs` — and the HTTP-level tests
of `crates/conformance/tests/component_http.rs` driving `wasi:http/handler` through
`wasmtime_wasi_http::p3::bindings::Service`.

The stub carries no `Send`/`Sync` assertion, and its absence is the point. A world
with resources would need one: the guest puts a resource's wrapper type at
`nlipsy_core::serve`'s `Arc<Conformity<E, A>>: Sync` seam, and a `#[repr(transparent)]`
handle has to be proved `Send + Sync` there. There are no wrappers here: the host's five
interfaces are free functions, so the guest's adapters are zero-sized types and the
bound is proved by their being empty.

**The file produces the TypeScript the package is written against.**

```sh
jco types wit/ --world-name nlipsy -o <out>
```

`jco types` takes the **WIT directory** and needs no component, so it runs with no
package in the tree. §5 is what it renders and what the npm package's flattening is
written against.

## 3. The seed table

Every export and every host import, with the Rust item it is lifted from. Paths are
relative to the repository root. `types` declares no functions and is covered by the
first block.

### `interface types` — the vocabulary

| WIT | seed |
|---|---|
| `number` | `serde_json::Number` as `crates/nlipsy-protocol/src/content.rs::Content::Number` holds it |
| `content` | `crates/nlipsy-protocol/src/content.rs::Content`; the `structured` spelling is `crates/nlipsy-protocol/src/content.rs::Content::to_json_value` |
| `format` | `crates/nlipsy-protocol/src/message.rs::Format`, ECMA-430 Table 1; the order from `crates/nlipsy-protocol/src/message.rs::Format::ALL` |
| `submessage` | `crates/nlipsy-protocol/src/message.rs::Submessage` |
| `message` | `crates/nlipsy-protocol/src/message.rs::Message` |
| `wire` | `crates/nlipsy-protocol/src/content.rs::Wire` |
| `parse-error-kind` | the corpus's kinds (`conformance/README.md`, "Expectations") as `crates/conformance/src/lib.rs::describe` maps them; `unmapped` from the same function |
| `parse-error` | `crates/nlipsy-protocol/src/error.rs::ParseError`; `path` from `crates/nlipsy-protocol/src/error.rs::ParseError::path`; `field` from `crates/conformance/src/lib.rs::ErrorDescription` |
| `severity` | `crates/nlipsy-protocol/src/error.rs::Severity` |
| `violation` | `crates/nlipsy-protocol/src/error.rs::Violation` |
| `disposition` | `crates/nlipsy-core/src/conformity.rs::Disposition` |
| `finding` | `crates/nlipsy-core/src/conformity.rs::Finding` |
| `token-ceiling` | `crates/nlipsy-core/src/conformity.rs::Rejected::TooManyTokens` and its two named fields |
| `rejected` | `crates/nlipsy-core/src/conformity.rs::Rejected` |
| `finished` | `crates/nlipsy-core/src/conformity.rs::Finished`; `withheld` from `crates/nlipsy-core/src/conformity.rs::Finished::withheld` |
| `token-kind` | `crates/nlipsy-core/src/token.rs::TokenKind` |
| `context` | `crates/nlipsy-core/src/context.rs::Context` and its two named fields; `exchange` replaces `crates/nlipsy-core/src/context.rs::Context::extensions`, which is a Rust type map and cannot cross |
| `layer-step` | `crates/nlipsy-core/src/middleware.rs::Middleware`, split around `crates/nlipsy-core/src/middleware.rs::Next`, whose `crates/nlipsy-core/src/middleware.rs::Next::run` is what `pass` continues into and `answer` does not |
| `config` | `crates/nlipsy-core/src/conformity.rs::ConformityConfig` |
| `policy` | `crates/nlipsy-core/src/conformity.rs::Policy` |
| `rejection` | `crates/nlipsy-core/src/conformity.rs::Rejection` |
| `status-map` | `crates/nlipsy-http/src/status.rs::StatusMap` |
| `cors` | **none — the binding's own** (§1): no cross-origin header at all unless the request carried an `Origin` that matches |
| `http-options` | `crates/nlipsy-server/src/server.rs::ServerConfig`; the limit is `crates/nlipsy-http/src/binding.rs::DEFAULT_MAX_REQUEST_BODY_SIZE` and the header `crates/nlipsy-http/src/binding.rs::CORRELATOR_HEADER` |
| `unavailable` | `crates/nlipsy-core/src/endpoint.rs::EndpointError::Unavailable` |
| `endpoint-error` | `crates/nlipsy-core/src/endpoint.rs::EndpointError` |
| `outcome` | `crates/nlipsy-core/src/serve.rs::Outcome` |
| `owed` | `crates/nlipsy-core/src/serve.rs::Owed` |
| `served` | `crates/nlipsy-core/src/serve.rs::Served` |
| `http-detail`, `exchange-detail` | `crates/nlipsy-core/src/transport.rs::ExchangeDetail`; the delay is read by `crates/nlipsy-http/src/retry.rs::parse_retry_after` |
| `exchange` | `crates/nlipsy-core/src/transport.rs::Exchange` |
| `transport-failure`, `transport-error` | `crates/nlipsy-core/src/transport.rs::TransportError` |
| `opaque`, `body` | `crates/nlipsy-core/src/transport.rs::Body`; the preview length is `crates/nlipsy-core/src/transport.rs::OPAQUE_PREVIEW` |
| `reply` | `crates/nlipsy-core/src/transport.rs::Reply`; `text` from `crates/nlipsy-core/src/transport.rs::Reply::text`, `is-error` from `crates/nlipsy-core/src/transport.rs::Reply::is_error` |
| `reply.tokens` | `crates/nlipsy-core/src/transport.rs::Reply::tokens`; the first submessage is chained in by `crates/nlipsy-core/src/first_submessage.rs::first_submessage_token` |
| `reply.conversation-tokens`, `reply.authentication-tokens` | `crates/nlipsy-core/src/transport.rs::Reply::conversation_tokens`; `crates/nlipsy-core/src/transport.rs::Reply::authentication_tokens`; both are derived from `tokens` above, so the three cannot disagree |

### `export interface protocol` — ECMA-430 clause 5

| WIT | seed |
|---|---|
| `decode` | `crates/nlipsy-protocol/src/message.rs::Message::from_json_bytes`; `crates/nlipsy-protocol/src/message.rs::Message::from_cbor_slice` |
| `encode` | `crates/nlipsy-protocol/src/message.rs::Message::to_json_vec`; `crates/nlipsy-protocol/src/message.rs::Message::to_cbor_vec`; the canonical JSON string is `crates/nlipsy-protocol/src/message.rs::Message::to_json_string` |
| `validate` | `crates/nlipsy-protocol/src/message.rs::Message::validate` |
| `is-control` | `crates/nlipsy-protocol/src/message.rs::Message::is_control`; the routing use is `crates/nlipsy-core/src/dispatch.rs::Dispatcher::route` |
| `extract-text` | `crates/nlipsy-protocol/src/message.rs::Message::extract_text` |
| `first-submessage-token` | `crates/nlipsy-core/src/first_submessage.rs::first_submessage_token` |
| `token-count` | `crates/nlipsy-core/src/conformity.rs::token_count` |
| `token-kind-of` | `crates/nlipsy-protocol/src/message.rs::Submessage::is_conversation_token`; `crates/nlipsy-protocol/src/message.rs::Submessage::is_authentication_token`; the three-way classification is `crates/conformance/src/lib.rs::canonical_tokens` |
| `upload-request` | `crates/nlipsy-protocol/src/message.rs::Message::upload_request` |
| `upload-response` | `crates/nlipsy-protocol/src/message.rs::Message::upload_response` |

### `import interface host-endpoint` — the handler, and what it configures

State a host keys on `context.exchange` is keyed **per component instance**: the number
comes from a counter inside the instance and starts again at `1` in the next one, so a
host that pools instances against one shared store of session state must key on the
instance as well or it will merge two exchanges numbered `5`. Neither shipped host can
reach it — jco's glue is one instance per module and `crates/conformance`'s `HostState`
is per-store — and `types.context` says the same thing on the field.

| WIT | seed |
|---|---|
| `name` | `crates/nlipsy-core/src/endpoint.rs::Endpoint::name` |
| `conformity` | `crates/nlipsy-core/src/conformity.rs::ConformityConfig`; read per call, which is what `crates/nlipsy-core/src/conformity.rs::Conformity::new` is handed |
| `rejection-policy` | `crates/nlipsy-core/src/conformity.rs::Rejection` |
| `http-binding` | `crates/nlipsy-server/src/server.rs::ServerConfig` |
| `answer` | `crates/nlipsy-core/src/endpoint.rs::Endpoint::handle`, returning a `Message` exactly as the trait does: `finish` is at the bottom of the guest's own stack (`crates/nlipsy-core/src/serve.rs::Tail`) and no host produces a `Finished` |

### `import interface host-layers` — the deployment's layers

| WIT | seed |
|---|---|
| `count` | the length of the stack `crates/nlipsy-core/src/middleware.rs::Next::new` is given |
| `before` | `crates/nlipsy-core/src/middleware.rs::Middleware::call`, up to its `Next::run`; the `answer` case is the `Ok` a layer may return without calling it |
| `after` | the same `crates/nlipsy-core/src/middleware.rs::Middleware::call`, after its `Next::run` returns |

### `import interface host-authority` — §6.2's exception and §6.2.1's memory

| WIT | seed |
|---|---|
| `is-own` | `crates/nlipsy-core/src/token.rs::TokenAuthority::is_own` |
| `issued` | `crates/nlipsy-core/src/token.rs::TokenAuthority::issued` |
| `mint` | `crates/nlipsy-core/src/token.rs::TokenAuthority::mint` |

### `import interface host-transport`, `host-record-sink`

| WIT | seed |
|---|---|
| `host-transport.send` | `crates/nlipsy-core/src/transport.rs::Transport::exchange`; renamed because an interface has one namespace for its types and its functions, and the `exchange` record is in it |
| `host-record-sink.write` | `crates/nlipsy-core/src/record.rs::RecordSink::record`; the line's spelling is `crates/nlipsy-core/src/record.rs::Recorded::to_json_line` |

### `export interface engine` — clause 6 and the composed order

| WIT | seed |
|---|---|
| `default-config` | `crates/nlipsy-core/src/conformity.rs::ConformityConfig::default`; the ceiling is `crates/nlipsy-core/src/conformity.rs::DEFAULT_MAX_TOKENS_PER_MESSAGE` |
| `default-rejection` | `crates/nlipsy-core/src/conformity.rs::Policy::default`; `crates/nlipsy-core/src/conformity.rs::Rejection` derives it |
| `default-http-options` | `crates/nlipsy-server/src/server.rs::ServerConfig::default`, assembled in the guest from the three values that crate assembles it from, because it does not compile for `wasm32-wasip3`: `crates/nlipsy-http/src/binding.rs::DEFAULT_MAX_REQUEST_BODY_SIZE`, `crates/nlipsy-http/src/binding.rs::CORRELATOR_HEADER` and `crates/nlipsy-http/src/status.rs::StatusMap::default`. Reachable by a host that **instantiates** the component and calls its exports — the npm package does — and **not** by an end-point component composed with `wac plug`, which sees the engine's imports and none of its exports; a plugged end-point writes the eleven values out, and `examples/echo-endpoint/src/host.rs` is what that looks like |
| `owed-by` | `crates/nlipsy-core/src/serve.rs::owed` |
| `public-message` | `crates/nlipsy-core/src/serve.rs::Outcome::public_message`; the failure half is `crates/nlipsy-core/src/endpoint.rs::EndpointError::public_message` |
| `retry-after-of` | `crates/nlipsy-core/src/serve.rs::Outcome::retry_after`; the failure half is `crates/nlipsy-core/src/endpoint.rs::EndpointError::retry_after` |
| `error-reply` | `crates/nlipsy-core/src/serve.rs::error_reply` |
| `check` | `crates/nlipsy-core/src/conformity.rs::Conformity::check` |
| `finish` | `crates/nlipsy-core/src/conformity.rs::Conformity::finish` |
| `serve` | `crates/nlipsy-core/src/serve.rs::serve`; the step it runs unconditionally afterwards is `crates/nlipsy-core/src/conformity.rs::Conformity::close` |
| `serve-message` | `crates/nlipsy-core/src/serve.rs::serve_message`; the composer under both is `crates/nlipsy-core/src/serve.rs::serve_answered` |

### `export interface client` — the client half

| WIT | seed |
|---|---|
| `call` | `crates/nlipsy-core/src/transport.rs::Client::send`, over `crates/nlipsy-core/src/transport.rs::encode_request` and `crates/nlipsy-core/src/transport.rs::decode_reply`; the wire is `crates/nlipsy-core/src/transport.rs::Client::on_wire`; the request's shape is `crates/nlipsy-http/src/binding.rs::NLIP_PATH`, `crates/nlipsy-http/src/binding.rs::CORRELATOR_HEADER` and `crates/nlipsy-http/src/binding.rs::CONTENT_TYPE`, and the delay is `crates/nlipsy-http/src/retry.rs::parse_retry_after` |
| `call-through` | the same `crates/nlipsy-core/src/transport.rs::Client::send`, over the host's `crates/nlipsy-core/src/transport.rs::Transport` |
| `call-recording` | `crates/nlipsy-core/src/record.rs::Recorder`; the line is `crates/nlipsy-core/src/record.rs::Recorded::to_json_line` and what it holds is `crates/nlipsy-core/src/record.rs::Recorded::of` |
| `replay` | `crates/nlipsy-core/src/record.rs::Replay::from_jsonl`; the wire is `crates/nlipsy-core/src/record.rs::Replay::on_wire`; the error is `crates/nlipsy-core/src/record.rs::ReplayError` |

### `export wasi:http/handler@0.3.0` — ECMA-431 §6, in the guest

Not nlipsy's interface, so it has no row of its own; what it *does* has seeds, and they
are the ones the axum binding uses, because the two are two shells over one crate.

| behaviour | seed |
|---|---|
| the path and the method | `crates/nlipsy-http/src/binding.rs::NLIP_PATH`; `POST` is ECMA-431 §6's own word |
| both spellings of the path | `crates/nlipsy-server/src/server.rs::Server::router` mounts `NLIP_PATH` and the trailing-slash form together, because the ECMA-430 reference server mounts `/nlip/` while the reference client POSTs to `/nlip`, and serving both avoids depending on a redirect; the guest strips the slash rather than writing a second literal |
| the body limit, and the `413` | `http-options.max-request-body-size`, answered with `crates/nlipsy-http/src/message.rs::too_large` at `status-map.too-large` |
| the status line | `crates/nlipsy-http/src/status.rs::StatusMap::of` |
| `Retry-After` | `crates/nlipsy-http/src/retry.rs::retry_after_value` |
| `Content-Type` | `crates/nlipsy-http/src/binding.rs::CONTENT_TYPE` |
| the correlator, echoed or minted | `crates/nlipsy-server/src/server.rs::ServerConfig::correlator_for`; the bound is `crates/nlipsy-server/src/server.rs::MAX_CORRELATOR_LENGTH`, which is that binding's number and this one takes it |
| another method on the path | `crates/nlipsy-http/src/message.rs::method_not_allowed`, at `405` with `Allow: POST` — the header has no seed and none is wanted: its value is the method ECMA-431 §6 names, and a constant for it would be an export nothing in Rust reads |
| any other path | `crates/nlipsy-protocol/src/message.rs::Message::error`, at `404`, so that a wrong path gets an NLIP error body rather than a runtime's plain text: one body shape for every answer |
| `OPTIONS` and the cross-origin headers | `types.cors`, which has no Rust seed (§1), under the rule stated there |
| the reply's framing | `content-length` from the encoded body, as the guest's own client half sets it on a request and as `crates/nlipsy-server/src/reply.rs::json_response` gets it from hyper, so a peer can apply its own bound before reading a byte; `204` carries none |
| `context.peer` | **no seed, and none possible**: `wasi:http@0.3.0` carries no remote address, so the guest answers `none`. The axum binding fills it from `ConnectInfo` — `crates/nlipsy-server/src/server.rs::Peer::label` — and `types.context.peer` says a host must read `none` as "this binding does not know" |

## 4. The corpus table

`conformance/cases/` is 125 cases in two kinds. Every field either maps to a path in
this contract or is derived by the runner from something that does; this table is the
proof, and the rows marked **derived** say what from.

`conformance/README.md`, "Running the corpus" and "Running an exchange case, in any
language", is the algorithm this table serves.

### A message case — 101 of them

| case field | WIT path, or how the runner derives it |
|---|---|
| `wire` | `types.wire` — `'json' \| 'cbor'` |
| `input` | the text, UTF-8-encoded by the runner → `protocol.decode(bytes, wire)` |
| `input_hex` | the hex, decoded by the runner → the same call. 12 cases |
| `expect.decode: "error"` | `decode` returns `err` (jco: it throws — §5) |
| `expect.error.kind` | `parse-error.kind`. 29 cases, and all eight kinds occur: `missing-field` 7, `cbor` 5, `wrong-type` 5, `base64` 4, `not-object` 3, `unknown-format` 2, `duplicate-field` 2, `invalid-json` 1 |
| `expect.error.path` | `parse-error.path` |
| `expect.error.field` | `parse-error.field` |
| `expect.decode: "ok"` | `decode` returns `ok` |
| `expect.message` | **derived** — the canonical model is the corpus's own projection of the decoded value, and the runner builds it from the `message` record: omit absent optionals, `format` is already the lower-case Table 1 value, `submessages` omitted when empty |
| `expect.message` — `{"$bytes": …}` | **derived** — `content.bytes` (a `Uint8Array`) base64-encoded by the runner. It is distinguishable from `content.text` at the boundary, which is the whole point of the marker |
| `expect.violations[]` | `protocol.validate(message)` → `violation.{rule, severity, path}` |
| `expect.is_control` | `protocol.is-control(message)` |
| `expect.text` | `protocol.extract-text(message, none)` |
| `expect.tokens[]` | **derived** — `message.submessages` filtered on `format == 'token'`, in order. The filter is a comparison on a field this contract carries, not a rule: `crates/nlipsy-protocol/src/message.rs::Submessage::is_token` is that equality and nothing else |
| `expect.tokens[].subformat`, `.content` | the `submessage` record's own fields |
| `expect.tokens[].kind` | `protocol.token-kind-of(submessage)`, with `none` as the corpus's `"other"` |
| `expect.token_count` | `protocol.token-count(message)`. The first submessage is **included**, which is what makes it a different question from the length of `expect.tokens` — and what the token ceiling counts |
| `expect.first_submessage_token` | `protocol.first-submessage-token(message)` → `option<submessage>`; an explicit `null` in the case asserts `none` |
| `expect.canonical_json` | `protocol.encode(message, 'json')`, UTF-8-decoded. 72 cases |

### An exchange case — 24 of them

| case field | WIT path, or how the runner derives it |
|---|---|
| `request` | the JSON object, re-encoded by the runner → `protocol.decode` |
| `endpoint.config.*` | `types.config`, field for field, handed to the guest by `host-endpoint.conformity` |
| `endpoint.config` absent | `engine.default-config()` — the numbers a runner would otherwise re-type |
| `endpoint.rejection.{inbound,outbound}.{on_error,on_warning}` | `types.rejection` → `types.policy` → `types.disposition`, handed to the guest by `host-endpoint.rejection-policy` |
| `endpoint.rejection` absent | `engine.default-rejection()` |
| `endpoint.authority.own` | the host's `host-authority.is-own` answers membership |
| `endpoint.authority.issued` | the host's `host-authority.issued`, per `token-kind` |
| `endpoint.authority.mint` | the host's `host-authority.mint`, per `token-kind`; a value it returns joins `own` and `issued` from that moment, which is the host's to remember |
| `endpoint.reply` | the JSON object → `protocol.decode` → `engine.finish(request, reply, cx)` |
| `endpoint.reply` absent | the case's outcome is `refused`; `engine.check` refuses before a reply exists |
| `expect.outcome: "reply"` | `engine.check` returned `ok` |
| `expect.outcome: "refused"` | `engine.check` returned `err(rejected)` |
| `expect.refused.kind` | the `rejected` case tag: `violations` or `too-many-tokens` |
| `expect.refused.rules[]` | `rejected.violations[].rule`, as a set |
| `expect.refused.count`, `.limit` | `rejected.too-many-tokens`'s `token-ceiling`, field for field |
| `expect.reply.messagetype` | `finished.message.message-type`; an explicit `null` asserts it is absent, which `option<string>` distinguishes |
| `expect.reply.control` | `finished.message.control`, the same way |
| `expect.reply.first_submessage` | `finished.message.{format, subformat}` |
| `expect.reply.tokens[]` | **derived** — `finished.message.submessages` filtered on `format == 'token'`, compared as a multiset by `{format, subformat, content, label}` |
| `expect.reply.contains[]` | **derived** — the same list, membership by `{format, subformat}` |
| `expect.reply.canonical` | **derived** — the canonical model of `finished.message`, as for a message case |
| `expect.findings[].rule`, `.severity`, `.path` | `finding.violation.{rule, severity, path}` |
| `expect.withheld` | `finished.withheld` from `engine.finish` — and on a `short_circuit` case `served.withheld`, which is `false` there because no reply of the end-point's own was finished. Not derivable from `expect.reply`: a replacement error message and a handler's own error message are the same bytes |
| `endpoint.short_circuit` | `engine.serve-message(request, cx)`, with **one host layer** whose `before` answers: `host-layers.count()` is `1` and `host-layers.before(0, …)` returns `layer-step.answer(endpoint.reply)`. The composed order then runs `close` over it and the handler is never asked — which is the *same route a Rust deployment takes*, and the reason no host can present a `finished` it built by hand |
| `expect.findings[]` on a `short_circuit` case | `served.findings`, which is `check`'s alone there: the layer answered, so `finish` never ran and reported none. They are the `inbound` ones |
| `expect.findings[].direction` | **not derivable from `served.findings`** — `inbound` is what `engine.check` returned, `outbound` is `finished.findings`. The runner drives the two steps itself, which is what `conformance/README.md` steps 3 and 4 prescribe and what `crates/conformance/src/exchange.rs` does |

Three fields deserve their own sentence.

**`expect.findings[].direction`.** `served.findings` is one list — the request's
findings from `check` first, then the reply's from `finish`, as
`crates/nlipsy-core/src/serve.rs::serve_answered` concatenates them — with no marked
boundary and no count of either half. It is *not* recoverable. Three exchange cases
carry a non-empty `findings` today and each has exactly one entry, so a runner driving
`serve` alone would pass all three by accident; a case with one finding in each
direction would be ambiguous, and the corpus exists to grow.

**`expect.reply.*` through `check`+`finish` rather than through `serve`.** The Rust
runner never calls `serve` for an ordinary exchange case: clause 6 is two synchronous
steps over values and an exchange case is those two steps applied to data
(`crates/conformance/src/exchange.rs`, module doc). A runner in another language does
the same, which is also why an exchange case needs no endpoint, no executor and no
context that carries anything.

**`endpoint.short_circuit` is the one case kind that drives the composed order**, and it
drives it the way a deployment does. Layers are the guest's, so the case is a host layer
whose `before` answers: `engine.serve-message` runs `check`, asks layer 0, gets
`answer(reply)`, never reaches the handler or `finish`, and runs `close` over the reply.
The three idempotent obligations are applied and the sanitiser and the mint are not,
which is exactly what `readings/51`–`/55` assert. There is no other way to reach the
shape: the host's `answer` returns a `message`, `engine.finish` is the only producer of a
`finished`, and so no host can present a reply that skipped the step.

## 5. What jco renders

**Measured on 2026-10-21, against jco 1.39.0 and `@bytecodealliance/preview3-shim`
0.11.0, through the npm package in `javascript/`.** The transpile of this world parses,
instantiates and runs in Node 26.5.0 and Bun 1.4.0: every export, all five host imports,
and the whole conformance corpus through the package. Three seams trap and only three —
`wasi:http/handler`, `wasi:http/client`, and a host callback suspended across a turn of
the event loop — and their suites are the package's `make check-js-e2e`, red by design
until jco follows (`javascript/README.md`, *What runs today, and what does not*).

The contract is a small thing to render, and the three lines that matter are these:
**no resources**, **the context by value**, and **every export either `async func` or a
plain `func`** — no constructors, no `Symbol.dispose`, no handle tables, no borrow
lowering. jco's `--import-bindings optimized` still panics on any wasip3 component
(`unexpected non-host resource table`), which is a defect of its own and not this
contract's.

Two things the measurement corrected. **Every export returns a `Promise` at run time,
`func` included** — `default-config()` comes back as a promise even though the `.d.ts`
types it as returning the record — so the package awaits every call, and a caller of the
transpile directly must too. And **doc comments are not copied into the `.d.ts`**: the
generated declarations carry a `@module` line per interface and a `# Variants` block per
`enum`, nothing per function, record or field, so the package's `types.ts` restates the
contract's documentation and cites `nlipsy.wit` for it.

| WIT | TypeScript |
|---|---|
| `record` | `export interface X { … }`, fields camelCased: `message-type` → `messageType`, `retry-after-ms` → `retryAfterMs`, `max-request-body-size` → `maxRequestBodySize` |
| `option<T>` in a record | an **optional property**: `retryAfterMs?: bigint`. `none` is an absent key or `undefined` |
| `option<T>` in a signature | `T \| undefined` |
| `enum` | a string union: `export type Format = 'text' \| 'token' \| …`; `Owed` is `'nothing' \| 'clause6' \| 'control-only'` |
| `variant` | a union of `{ tag, val }` interfaces, one named type per case: `LayerStep` = `LayerStepPass \| LayerStepAnswer`, each `{ tag: 'pass', val: Message }` |
| `%bool` (the escaped case) | `{ tag: 'bool', val: boolean }` — the escape is spelling only |
| a case with no payload | `{ tag: 'null' }`, `{ tag: 'ok' }` — no `val` |
| `list<u8>` | `Uint8Array` |
| `list<T>` | `Array<T>` |
| `u8`, `u16`, `u32`, `f64` | `number` |
| `u64`, `s64` | **`bigint`** — `context.exchange` is a `bigint`, and so is `EndpointErrorTimedOut.val`. `Date.now()` is a `number`; the wrapper converts |
| `string` | `string` |
| `result<T, E>` | **`T`, and the error is thrown** — a `ComponentError` whose non-enumerable `payload` is the error value |
| `result<_, E>` | `void`, the error thrown |
| `func` | `export function name(…): T` in the `.d.ts` — and a `Promise<T>` at run time for this component; the package awaits it |
| `async func` | `export function name(…): Promise<T>` |
| the world | `nlipsy.d.ts` re-exports: the five host interfaces as `export type * as …`, the exports as `export * as protocol` / `engine` / `client` |

Three things in that output are worth naming.

- **`Number` is shadowed.** `interfaces/nlipsy-engine-types.d.ts` declares
  `export type Number = NumberInt | NumberUint | NumberFloat`, which shadows the
  JavaScript global inside every module that imports `*` from it.
- **`option<u64>` becomes `bigint | undefined`**, so `some(0)` is `0n` and `none` is
  `undefined` — the distinction `http-detail.retry-after-ms` depends on survives, but
  only if the wrapper does not normalise falsy values. `context.exchange` has the
  matching trap: `0n` is the value that means "no exchange named" and is falsy.
- **The thrown-error convention is invisible in the types.** Every `result` in
  this contract is a `try`/`catch` on the JavaScript side, and every *host* callback
  the wrapper supplies must convert a thrown `Error` into the WIT error shape, because
  jco's `getErrorPayload` re-throws an `instanceof Error` rather than lowering it.

## 6. How the contract changes

The version is `nlipsy:engine@0.1.0`, in the package declaration, and it is the
contract's version rather than any crate's. The npm package in `javascript/`
has its own semver and its changelog names both the crate versions the component was
built from and this package version.

**What is a breaking change.** In the Component Model an interface is matched
structurally, so *every* one of these breaks a compiled consumer:

- removing or renaming an interface, a function, a type, a record field, a variant case
  or an enum case;
- **adding** a case to a `variant` or an `enum`, or a field to a `record` — a consumer
  that does not know the new member cannot lift a value carrying it;
- changing a type anywhere in a signature, including `option<T>` to `T`;
- turning a `func` into an `async func` or back — the lowering and the ownership of its
  parameters both change, so a consumer's glue no longer matches the export.

The asymmetry is worth stating plainly: the Rust types behind this
contract are almost all `#[non_exhaustive]`, where adding a variant is deliberately
*not* breaking. Here it is. The two most likely to move first are `status-map` (a
deployment's codes, and `StatusMap` in Rust is deliberately not `#[non_exhaustive]` so
that struct-update syntax works) and `exchange-detail` (one case today; ECMA-432 and
ECMA-433 each add one when their bindings arrive).

**What is additive.** Adding a *new* interface to the package, or a new function to an
existing interface. A consumer compiled against the older interface still matches.
`StatusMap::of_error` in `crates/nlipsy-http/src/status.rs` is the standing example: the
`Outcome::Failed` arm on its own, which ECMA-431 §6.1's out-of-band upload leg would
need and which nothing in the guest reaches today, so it can be added the day an upload
sink needs it without moving anything. So can a `request` constructor for TR/113 §11.1's
chat turn and the `conversation` subformat beside it, both in
`crates/nlipsy-http/src/message.rs`: they are a client's convenience, nothing in the
guest calls them, and they come back as free functions the day a caller asks.

**What is free.** Doc comments. Nothing downstream of them is compiled against their
text: the Rust is regenerated from this directory, and jco copies none of them into its
`.d.ts` (§5), so improving one is a rebuild and nothing more. The one cost is on the
JavaScript side rather than on this one — the npm package's `types.ts` restates these
sentences and has to be edited with them — which is still the reason to keep writing
them here rather than in a second document that can drift.

**The procedure.** A breaking change moves the package version — `0.2.0` while the
contract is pre-1.0, and every version thereafter by semver — and the same version
appears in three places that must agree: this file's `package` line, the guest's
`wit_bindgen::generate!` (which reads this directory, so it follows automatically), and
the `--map 'nlipsy:engine/host-endpoint=…'` arguments `javascript/scripts/build.mjs`
passes to `jco transpile`, which name an interface without its version and therefore do
not. Run
the checks of §2 before committing one; the stub build among them is what catches a
change wit-bindgen cannot generate code for, which is the class of mistake parsing
alone misses.

**Before the first publication, the version is open.** Nothing consumes
`nlipsy:engine@0.1.0` from a registry yet — the npm package in `javascript/` is
unpublished and the component is built from this tree — so there is no compiled consumer
for a breaking change to break, and moving the version would only spend a number. A
breaking change taken under that rule leaves the `package` line where it is; the same
change after the first publication is a version bump. This paragraph stops applying the
day the package is published.
