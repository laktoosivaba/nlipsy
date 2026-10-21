# nlipsy

[![crates.io](https://img.shields.io/crates/v/nlipsy.svg)](https://crates.io/crates/nlipsy)
[![docs.rs](https://img.shields.io/docsrs/nlipsy)](https://docs.rs/nlipsy)
[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)
[![licence](https://img.shields.io/crates/l/nlipsy.svg)](https://github.com/laktoosivaba/nlipsy#licence)

NLIP — the Natural Language Interaction Protocol — is the Ecma International standard
(ECMA-430) for agents that exchange messages written in natural language. ECMA-431,
ECMA-432 and ECMA-433 carry those same messages over HTTP, WebSocket and AMQP.

This crate is the facade: it contains no NLIP code of its own and re-exports the
`nlipsy-*` crates as modules, each behind a feature flag.

```toml
[dependencies]
nlipsy = { version = "0.1", features = ["client"] }   # or "server", or both
```

## Why a facade

NLIP is one message model, a runtime core over it and several transports, so the Rust
for it is several crates: a binding that lived inside the model would make every
consumer compile it, and a binding that lived inside another binding would make the
second depend on the first. Splitting them is the right shape for the code and the
wrong shape for the manifest of whoever uses it — a consumer who starts with the
message model and later speaks HTTP should not have to change which crate they depend
on.

So the split stays, and this crate is the one line that hides it:

```toml
[dependencies]
nlipsy = { version = "0.1", features = ["protocol"] }
```

```rust,ignore
use nlipsy::protocol::{Format, Message, Submessage};

let question = Message::from_json_str(
    r#"{"format": "text", "subformat": "english", "content": "What is Ecma?"}"#,
)
.unwrap();

assert_eq!(question.format, Format::Text);

let answer = Message::text("An international standards body.")
    .with_submessage(Submessage::token("conversation", "c1"));

assert!(answer.to_json_string().contains("conversation"));
```

### How the pieces fit

`nlipsy-protocol` is clause 5: what a message *is*, on the JSON wire and the CBOR one.
It knows nothing about exchanges. `nlipsy-core` is clause 6 over it: what an end-point
*owes* — the token echo, the control carry, the issuance rules — expressed as
synchronous functions over values, plus the seams a runtime plugs into (`Endpoint`,
`TokenAuthority`, `Middleware`, `Transport`). It has no executor, no socket and no
clock, which is the test that decides what belongs in it, and it is why the same code
will compile for a JavaScript package. The bindings are the crates that *do* have a
socket: `nlipsy-http-client` is ECMA-431 from a client agent's end, `nlipsy-server`
from a server agent's, and `nlipsy-http` is what those two must agree on — ECMA-431 §6
as values, with no socket of its own. This crate is the sixth, over all five.

`nlipsy::protocol::Message` *is*
[`nlipsy_protocol::Message`](https://docs.rs/nlipsy-protocol): one type, re-exported,
not a wrapper around one. Depending on
[`nlipsy-protocol`](https://crates.io/crates/nlipsy-protocol) directly is equally
supported and always will be — it is the crate the work is in, it has no dependency
on this one, and a library that only ever parses messages has nothing to gain here.
The facade is for the manifest, not for the API.

## Features

There are **no default features**. `nlipsy = "0.1"` on its own compiles an empty
crate; every module is asked for by name. One line each, what it gives you:

| feature | enables | what you get |
|---|---|---|
| `protocol` | `nlipsy-protocol` | `nlipsy::protocol` — the message model: `Message`, `Submessage`, `Format`, `Content`, and ECMA-430 clause 5's rules about them |
| `client-core` | `protocol`, `nlipsy-core/client` | `nlipsy::core` — the outbound half: `Transport`, `Client`, `Reply`, `Context`, record and replay. No socket in it; a binding supplies one |
| `http-client` | `client-core`, `nlipsy-http-client/reqwest`, `nlipsy-http` | `nlipsy::http_client` — ECMA-431 from the client's end: `HttpTransport` over reqwest, verifying against the platform's trust store, plus the file-backed recorder — and `nlipsy::http` with it |
| `http-client-hyper` | `client-core`, `nlipsy-http-client/hyper`, `nlipsy-http` | the same module and the same binding, with `HyperTransport` instead: hyper + rustls on `ring`, roots compiled in. It does **not** imply `http-client`, so asking for it alone compiles no reqwest — which is the point, and is what an Android or an iOS build over FFI asks for |
| `client` | `http-client` | every client *binding* there is. One today; write this when you mean "a client agent", and the next one arrives without a manifest edit. Not `http-client-hyper`, which is the same binding over another stack rather than another binding |
| `server-core` | `protocol`, `nlipsy-core/server` | `nlipsy::core` — the inbound half: ECMA-430 clause 6 as `Conformity`, plus the `Endpoint`, `TokenAuthority` and `Middleware` seams and the `Dispatcher` |
| `server` | `server-core`, `nlipsy-server`, `nlipsy-http` | `nlipsy::server` — ECMA-431 from the server's end: an `axum::Router` for `POST /nlip` with clause 6 underneath it, `ServerConfig`, the §6.1 upload leg — and `nlipsy::http` with it |
| `cbor` | `protocol`, and the CBOR codec of every crate above that is in the build | `Message::from_cbor_slice` and `Message::to_cbor_vec` — the CBOR encoding of ECMA-432 §7.1 — and a runtime that can carry it |
| `full` | `client`, `server`, `cbor` | every NLIP capability. Not `http-client-hyper`, for the reason `client` is not: a second way to build one binding is not a third thing the crate does |

A default would have to guess which piece of NLIP a consumer wants, and a wrong guess
is a dependency they cannot drop without turning `default-features` off — which is
the state a facade exists to spare them.

`nlipsy::core` is one of two modules that two features produce, because `nlipsy-core`
holds both halves of the runtime and a deployment that federates — answering requests
*and* asking peers — wants both. Naming both features is supported and yields the module
once; this is jsonrpsee's `cfg_client_or_server!`, and there is no bare `core` feature
for the same reason it has none: a core with neither half on is an empty crate.

`nlipsy::http` is the second, and for the mirror-image reason: ECMA-431 §6 as values —
the status map, the path, the correlator header, `Retry-After` in both directions, the
error bodies the binding produces before clause 6 is reached — is what a client agent
and a server agent *share*, one reading what the other wrote. So `http-client`,
`http-client-hyper` and `server` each bring it, it is emitted once however many are
named, and there is no `http` feature of its own: a consumer asks for the role they are
playing and the values come with it.

`nlipsy::http_client` is the third, and the first whose two features are not two roles.
`http-client` and `http-client-hyper` are one binding over two HTTP stacks, and they are
two features rather than one knob because the difference *is* a dependency graph: the
reqwest transport's TLS depends on rustls-platform-verifier unconditionally, which means
`jni` plus a Kotlin companion library on Android and a cmake-built BoringSSL everywhere,
and a feature cannot remove a dependency. Naming both gives the module once with both
transports in it; naming only `http-client-hyper` gives a build with no reqwest in it at
all. `make check-mobile-graph` is the gate that keeps that true, and
[`nlipsy-http-client`](https://docs.rs/nlipsy-http-client)'s own README has the measured
graph and what the second transport gives up — the system trust store, and with it
enterprise roots, platform proxy settings and pinning.

`cbor` reaches **every** crate in the build and not only the message model. That is
what the `?` entries in the manifest are for: `nlipsy-core`'s `encode_request` and
`decode_reply` compile a panicking arm for `Wire::Cbor` unless the core's own `cbor`
is on, so a `cbor` that stopped at `nlipsy-protocol` would give a consumer a model
that speaks CBOR behind a client that panics on it. The entries are weak (`?`), so
`cbor` still does not *pull in* a crate no other feature asked for.

## Module map

| module | feature | is |
|---|---|---|
| `nlipsy::protocol` | `protocol` | [`nlipsy-protocol`](https://docs.rs/nlipsy-protocol) |
| `nlipsy::core` | `client-core` or `server-core` | [`nlipsy-core`](https://docs.rs/nlipsy-core) |
| `nlipsy::http` | `http-client`, `http-client-hyper` or `server` | [`nlipsy-http`](https://docs.rs/nlipsy-http) |
| `nlipsy::http_client` | `http-client` or `http-client-hyper` | [`nlipsy-http-client`](https://docs.rs/nlipsy-http-client) |
| `nlipsy::server` | `server` | [`nlipsy-server`](https://docs.rs/nlipsy-server) |

Each module *is* the crate, re-exported — `nlipsy::protocol::Message` is
`nlipsy_protocol::Message`, one type and not a wrapper around one — so every link
above lands on the documentation that governs.

## The two roles

A client agent (ECMA-430 §6.1, ECMA-431 §6): build a message, build the context the
exchange happens in, send it.

```rust,ignore
use nlipsy::core::{Client, Context};
use nlipsy::http_client::HttpTransport;
use nlipsy::protocol::{Message, Submessage};

let request = Message::text("What is Ecma?")
    .with_submessage(Submessage::token("conversation", "c-1"));
let cx = Context::new().with_correlator("turn-1");

let client = Client::new(HttpTransport::new("https://agent.example.com"));
let answer = client.send(&request, &cx).await?.text();
```

A server agent: an endpoint, the authority that answers §6.2's exception, a router.

```rust,ignore
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

struct NoTokens;

impl TokenAuthority for NoTokens {
    fn is_own(&self, _token: &str, _cx: &Context) -> bool {
        false
    }
}

// The only constructor: clause 6 is underneath the router by construction.
let server = Server::new(Echo, NoTokens, ServerConfig::new());
let app = server.router();
```

Both examples are fenced `ignore`, and so is the one above them. This file is also the
crate documentation, so a doctest in it would be compiled by `cargo test` with **no
features on** — where none of these modules exists. The compiled copies live on the
modules themselves, where the feature that makes the code compile is the feature that
makes the module exist; they run under `cargo test --all-features`.

## What is not here yet

The features this table is expected to grow, named here so that the shape is on record
rather than improvised later:

- **`ws` / `ws-client`** — ECMA-432, NLIP over WebSocket: the `/nlip/ws` endpoint, CBOR
  frames, the `/nlip/ws/text` fallback, and §10's optional session management. Its
  client half joins `client`, its server half joins `server`, the way `http-client`
  and `server` do now.
- **`amqp`** — ECMA-433, NLIP over AMQP.
- **`macros`** — jsonrpsee has one and this crate does not, because NLIP has nothing to
  generate: a message names no method, so there is no trait of RPC signatures to derive
  a client and a server from.

None of those features is declared until the crate behind it exists. A feature that
enables nothing is indistinguishable from one that is broken, so `features = ["ws"]`
is an error today rather than a silent success.

## Minimum supported Rust version

1.85, the edition 2024 floor, the same as every crate in this workspace — including
the `http-client-hyper` graph, whose highest floor is hyper-rustls's own 1.85. Raising
it is treated as a breaking change while the version is 0.x.

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

ECMA-430, ECMA-431, ECMA-432, ECMA-433 and TR/113 are publications of Ecma
International and are not redistributed here.
