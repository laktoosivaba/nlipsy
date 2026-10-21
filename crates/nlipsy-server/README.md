# nlipsy-server

[![crates.io](https://img.shields.io/crates/v/nlipsy-server.svg)](https://crates.io/crates/nlipsy-server)
[![docs.rs](https://img.shields.io/docsrs/nlipsy-server)](https://docs.rs/nlipsy-server)
[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)
[![licence](https://img.shields.io/crates/l/nlipsy-server.svg)](https://github.com/laktoosivaba/nlipsy#licence)

The ECMA-431 server binding of NLIP: an axum router around a clause-6-conformant
end-point.

ECMA-431 §6 fixes two things and no more — the end-point lives at
`https://<server_name>:port/nlip` and accepts NLIP messages with `POST`. This crate is
that surface, with ECMA-430 clause 6 underneath it: it reads a body under a limit,
decodes a message, checks it, hands it to your endpoint through whatever layers your
deployment added, closes the obligations over the answer, and writes it back out.
Everything it decides that the standard does not — status codes, headers, error
bodies — it decides in a value you can replace.

## The only way in

There is no constructor that takes a bare `Endpoint` and no way to reach the router
without one:

```rust
use std::sync::Arc;
use nlipsy_core::{Context, Endpoint, EndpointError, TokenAuthority};
use nlipsy_protocol::message::Message;
use nlipsy_server::{Server, ServerConfig};

struct Greeter;

impl Endpoint for Greeter {
    fn name(&self) -> &str { "greeter" }

    async fn handle(&self, request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
        Ok(Message::text(format!("you said: {}", request.content_as_str().unwrap_or(""))))
    }
}

// §6.2's exception, answered by the deployment: this one creates no tokens, so it
// created none of the peer's either and every one of them comes back.
struct NoTokens;
impl TokenAuthority for NoTokens {
    fn is_own(&self, _token: &str, _cx: &Context) -> bool { false }
}

let server = Server::new(Greeter, NoTokens, ServerConfig::new());

// `POST /nlip` and `POST /nlip/`, and nothing else: no health routes, no fallback,
// no layers. A deployment merges its own onto it.
let app: nlipsy_server::axum::Router = server
    .router()
    .route("/health", nlipsy_server::axum::routing::get(|| async { "ok" }));
let _ = app;
```

`Conformity` is built *inside* `Server::new`, so your layers stack outside it and
cannot displace it. That is jsonrpsee's arrangement — its innermost service is always
its own `RpcService`, constructed by the server and never by the caller — and it
matters here because the echo is the obligation a reply built from scratch silently
drops.

A constructor is not the whole of it. A `Middleware` can answer `Ok` without calling
`Next::run` — a cache, a canned answer, a maintenance-mode layer — and never reach
`Conformity::finish`, and a layer's after-half holds the finished reply by value and
can take a token out of it. So `nlipsy_core::serve` — the composer this crate drives,
which owns the order the clause-6 steps run in — applies `Conformity::close` over
whatever the layer stack returned, unconditionally: the §6.2 echo, the §6.2.1 confirmation and the
§6.3 carry are the three idempotent obligations, and re-applying them costs three
linear scans on the ordinary path. **Nothing a layer does can remove what clause 6 owes
the peer; a layer can still break clause 5 after `finish`, and the outbound policy does
not see it.** That last one is your bug to find, not this crate's.

Serve the router the way you serve any other axum router. Mount it with
`into_make_service_with_connect_info::<SocketAddr>()` if you want `Context::peer` to
be the peer's address, which is what throttling and audit key on.

## The pipeline

| step | what it does | what decides |
|---|---|---|
| read the body | under `max_request_body_size`, **inside this crate** | `ServerConfig` |
| build the `Context` | peer from `ConnectInfo`, correlator from the header or minted, and the `Principal` a trusted proxy named, when one is configured | `ServerConfig` |
| decode | `Message::from_json_bytes`; failure is `Outcome::Malformed` | `nlipsy-core` |
| `Conformity::check` | clause 5 under the inbound policy, then §6.2's token ceiling | `Rejection`, `ConformityConfig` |
| your layers | `Middleware`, outermost first, with `&mut Context` | the deployment |
| your endpoint | one message in, one message or an `EndpointError` out | you |
| `Conformity::finish` | sanitise, §6.2 echo, §6.2.1 confirmation, §6.2.1/§6.2.2 minting, §6.3 carry | `nlipsy-core` |
| `Conformity::close` | the three idempotent obligations again, over whatever the layers returned | `nlipsy-core` |
| status and headers | `StatusMap::of(&outcome, withheld)`, `Retry-After`, `application/json`, the correlator echoed | `StatusMap`, `ServerConfig` |

Everything from *decode* to *close* is one call: `nlipsy_core::serve`, which returns a
`Served { reply, outcome, withheld, findings }` and never an error. The order is the
core's on purpose — `finish` at the bottom of the layer stack and `close` after it are
the two properties a second binding would re-derive and get wrong — and what is left
here is what the core has no opinion about: the socket, the body limit, the status
line, the headers and the log. `StatusMap` and the `Retry-After` spelling are
`nlipsy-http`'s, shared with `nlipsy-http-client`, which reads what this writes.

The body limit is enforced here rather than by a `tower-http`
`RequestBodyLimitLayer`, and that is the whole reason it is a `ServerConfig` field. A
limit layer outside the router refuses in its own voice — `length limit exceeded`, in
plain text — which is the one rejection that would break the principle this crate
inherited from the server it was read out of:

> a request that cannot be understood is answered with a non-2xx status *and* an NLIP
> `MessageType: Error` body, so a peer reading only the payload and one reading only
> the status line both learn what happened.

jsonrpsee moved the same check inside its own transport for the same reason
(`server/src/transport/http.rs`). Every refusal this crate can produce — malformed,
refused, too large, and each `EndpointError` — is an NLIP error message with the
request's tokens echoed into it wherever there was a request to take them from, and
with the conversation token this endpoint issued confirmed on it. The token ceiling is
the one refusal that echoes nothing, deliberately; see below.

## Layers, and why they are not `tower::Service`

A deployment's work — admitting a conversation, authenticating a principal, bounding
how long the endpoint may take — sits between the decoded message and the endpoint,
and it needs to *write* to the `Context` that clause 6 then reads. `nlipsy-core`'s
`Middleware` is that seam:

```rust
use nlipsy_core::{Context, EndpointError, Middleware, Next};
use nlipsy_protocol::message::Message;
use std::time::Duration;

/// The deployment's own: a bound on the endpoint, answered as `504`.
struct Deadline(Duration);

impl Middleware for Deadline {
    async fn call(
        &self,
        request: &Message,
        cx: &mut Context,
        next: Next<'_>,
    ) -> Result<Message, EndpointError> {
        // A real one wraps `next.run(..)` in its executor's timer; the error is the
        // point, and `StatusMap::timed_out` is what it becomes.
        match next.run(request, cx).await {
            Ok(reply) => Ok(reply),
            Err(error) => Err(error),
        }
    }
}
```

`tower::Layer` composes the HTTP edge and `axum::Router` is a `tower::Service`, so
both are already here. What is *not* a `tower::Service` is the message level, for the
reasons jsonrpsee gives for its own `RpcServiceT`: `poll_ready` has nothing to gate
when there is one handler at the bottom, `call` takes `&mut self` where a shared
endpoint wants `&self`, and a domain trait can hand a layer the request and the
context by reference rather than moving them into a future — which is exactly what
§6.2's echo needs, since it reads the request while writing the reply.

Layers are stacked as a list of trait objects rather than as a generic tower. One
allocation per layer per exchange, on a path that has already allocated a decoded
message, buys a `Server<E, A>` whose type does not grow with the stack and a
`Vec<Arc<dyn DynMiddleware>>` a `wasm32` target can hold.

`Conformity::finish` runs at the *bottom* of the stack, under every layer. §6.2.1's
token is minted inside `finish`, and a deployment that persists what it minted has to
run after that — so a layer's "after" half sees the reply that actually goes on the
wire, as jsonrpsee's layers see the final `MethodResponse`.

## A principal, when a proxy vouches for one

ECMA-434 §7.2.1 asks a profile-2 deployment to authenticate its clients, and TR/113 §9
allows it to "rely on just the base transfer protocol to perform the authentication" —
in an HTTP deployment, usually a gateway in front of the end-point that verifies a JWT
or a client certificate and writes whom it let through into a request header. Name that
header and the binding carries its value into the context, as a `Principal`, before any
layer runs:

```rust
use nlipsy_core::{Context, Endpoint, EndpointError, TokenAuthority};
use nlipsy_protocol::message::Message;
use nlipsy_server::axum::http::HeaderName;
use nlipsy_server::{Principal, Server, ServerConfig};

struct Whoami;

impl Endpoint for Whoami {
    fn name(&self) -> &str { "whoami" }

    async fn handle(&self, _request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        let who = cx.extensions.get::<Principal>().map_or("nobody", Principal::as_str);
        Ok(Message::text(who))
    }
}

struct NoTokens;
impl TokenAuthority for NoTokens {
    fn is_own(&self, _token: &str, _cx: &Context) -> bool { false }
}

let config = ServerConfig::new()
    .with_principal_header(Some(HeaderName::from_static("x-nlip-principal")));
let _router = Server::new(Whoami, NoTokens, config).router();
```

The binding verifies nothing, and that is the contract rather than a gap in it: the
value is worth exactly what the network makes it worth. It means something only when
every request reaches the router through the proxy that writes the header *and strips
any copy the caller sent* — a property of the network (a policy that admits only the
gateway), which no code here can check. The default names no header, so a deployment
that has not arranged that trusts nothing a caller sends. A header that arrives more than
once, is not visible ASCII, is empty, or is longer than `MAX_PRINCIPAL_LENGTH` carries no
principal at all, and the reason is logged without the value: a proxy that strips and
writes sends exactly one, so a second is a copy that got past it, and treating the
exchange as unauthenticated is safer than attributing it to either.

What the value *says* — a class of caller, an identifier, both — is between the proxy
and the deployment; `Principal` is opaque so that this crate does not settle a format
the standard leaves to them. The core never names the type: `Context::extensions` is
where, in its own words, "an authenticated principal travels without this crate having
to name any of them", and this binding is what puts one there.

## The upload leg is outside clause 6

`Server::upload_router(sink)` mounts `POST /nlip/upload` (ECMA-431 §6.1), whose
address a peer learns from a §6.4 control exchange. Nothing in clause 6 applies to it,
and that is stated rather than left to inference: the body is arbitrary bytes, so
there is no message to validate, no tokens to echo and nothing to finish. ECMA-430
§7.3 ¶2 calls it a base-transfer-protocol operation rather than a message exchange.

It is a router of its own because §6.1 asks for "any port different than the port used
by NLIP end point" — a route beside `/nlip` could only ever answer on `/nlip`'s port.
`DiscardSink` is the one implementation shipped; a filesystem or object-store sink
chooses a naming scheme and a failure policy, and both are a deployment's.

## What this crate decides, and what it refuses to

**It decides** the two routes, the media type, the correlator header's spelling, that
a refusal is an NLIP error message, and the default status codes: `400` malformed,
`400` refused, `413` too large, `501` unsupported, `503` unavailable with
`Retry-After`, `504` timed out, `500` internal, `500` withheld — the endpoint's own
reply broke clause 5 and was replaced, which is the endpoint failing and not the
exchange succeeding. Every one of them is a `StatusMap` field, because ECMA-431 fixes
none of them. A `GET` on `/nlip` is `405` with `Allow: POST` and an NLIP error message
as the body; §6 obliges `POST` and forbids nothing, so the status is this crate's
choice like the rest.

Every refusal echoes the request's tokens wherever there was a request to take them
from, confirms what this endpoint has already issued, and carries §6.3 — with **one
deliberate exception**: a request above `ConformityConfig::max_tokens_per_message` is
refused with no echo at all. Cloning what it refused is the work the ceiling exists to
refuse, so the departure from §6.2 ¶2 is taken knowingly, under ECMA-434 §7.2.5's
throttling recommendation rather than under clause 6, and at the default no conforming
exchange reaches it (§6.2 ¶3's example is two tokens per end-point).

**It refuses to decide** rate limiting (ECMA-434 §7.2.5 is a profile recommendation,
and a limiter that fires before this crate decodes anything answers through `reject`),
TLS — ECMA-430 §7.1 obliges it of any *deployment* ("must happen over a communication
channel that uses encryption") and a `Router` is served by whatever serves it —
tracing subscribers, health routes, session storage, how long an endpoint may take,
and what a token *is*. Those are a deployment's, and each of them reaches an
exchange as a layer, a route merged onto the router, or a `TokenAuthority`.

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
["server"] }` is this crate, under the name `nlipsy::server`.

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
