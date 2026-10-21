# nlipsy-http-client

[![crates.io](https://img.shields.io/crates/v/nlipsy-http-client.svg)](https://crates.io/crates/nlipsy-http-client)
[![docs.rs](https://img.shields.io/docsrs/nlipsy-http-client)](https://docs.rs/nlipsy-http-client)
[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)
[![licence](https://img.shields.io/crates/l/nlipsy-http-client.svg)](https://github.com/laktoosivaba/nlipsy#licence)

The ECMA-431 client binding of NLIP: one HTTP exchange with a peer end-point, over
reqwest or over hyper.

ECMA-430 §4.7 defines a *client agent* as "an agent which initiates the transfer of
data to another agent". `nlipsy-core` is that half of the protocol with no socket in
it — the `Transport` trait, `Client`, `Reply`, `Body` and the record/replay machinery.
This crate is that trait implemented: a `POST` to
`https://<server_name>:port/nlip` carrying one JSON message and returning one, which is
all ECMA-431 §6 asks for. Twice, over two HTTP stacks — see
[Two transports](#two-transports), which is a dependency-graph decision and not a
protocol one.

One exchange is one request. There is no stream, no subscription and no long-lived
connection: a conversation is a sequence of independent requests that happen to carry
the same identifier, which is why so little of this crate is about transport.

## What is here

| item | what it is |
|---|---|
| `HttpTransport` | `nlipsy_core::Transport` over reqwest: `POST`, `Content-Type: application/json`, the reply's bytes verbatim. Behind `reqwest`, the default feature |
| `HyperTransport` | the same contract over hyper + hyper-rustls on the `ring` provider, with the roots compiled in. Behind `hyper` |
| `HttpTransportBuilder`, `HyperTransportBuilder` | the connect budget and headers a deployment adds, one per transport and meaning the same two things; `HeaderMap` and `HeaderValue` are re-exported for both. `HyperTransportBuilder` has two more of its own: `dialer` and `add_root_certificate` |
| `Dialer`, `DialTarget`, `DialFuture`, `DialStream`, `DialedStream` | the seam under `HyperTransport`: a caller's own way of opening the byte stream — an in-process tunnel, a proxy, a pipe — so that the transport resolves no name and opens no socket itself. Behind `hyper`; see [Routing the connection yourself](#routing-the-connection-yourself) |
| `NLIP_PATH`, `CORRELATOR_HEADER`, `CONVERSATION_SUBFORMAT` | the path ECMA-431 §6 fixes and the correlation header this binding spells — `nlipsy-http`'s now, re-exported here |
| `request` | the TR/113 §11.1 chat turn: text in the first submessage, the conversation token beside it — also `nlipsy-http`'s, re-exported |
| `FileSink`, `Replay` | a recording on disk, and the same recording replayed — the file half of the core's `RecordSink` |
| `reqwest`, `hyper` (features) | one transport each. `default = ["reqwest"]`; both at once gives both types; neither gives the list above with no socket in it |
| `cbor` (feature) | forwards to `nlipsy-core/cbor`, for a consumer that also speaks ECMA-432 §7.1 |

```rust
use nlipsy_http_client::request;

// One exchange, over the default transport. `HyperTransport` goes in the same place and
// takes the same two arguments — see "Two transports" for which to pick. The module is
// `#[cfg]`-gated rather than written inline because this example is a doctest and the
// crate has a feature set with *neither* transport in it: the gate is what keeps the
// example compiling there, and naming the feature is also how a reader learns which one
// `HttpTransport` is behind.
//
// It is written out rather than run here: an `await` needs an executor, and a doctest
// that reached the network would be a test of somebody's uptime.
#[cfg(feature = "reqwest")]
mod over_reqwest {
    use nlipsy_core::{Client, Context, Reply, TransportError};
    use nlipsy_http_client::{HttpTransport, request};

    pub async fn ask(endpoint: &str, text: &str) -> Result<Reply, TransportError> {
        let client = Client::new(HttpTransport::new(endpoint));
        // The correlator is not an argument: ECMA-433 §6.1.4 makes correlation
        // normative over AMQP while ECMA-431 leaves it to convention, so it is one
        // concept with a spelling per binding and it travels in the context.
        let cx = Context::new().with_correlator("corr-1");
        client.send(&request(text, "english", "conv-1"), &cx).await
    }
}
#[cfg(feature = "reqwest")]
let _ = over_reqwest::ask;

// What goes on the wire, checked without one — and without a transport either, which is
// why this half of the example is not behind the gate. §6.2.1 lets either peer create
// the conversation token, and this client brings its own from the first turn.
let message = request("What is Ecma?", "english", "conv-1");
assert_eq!(message.submessages[0].content_as_str(), Some("conv-1"));
assert!(message.submessages[0].is_conversation_token());
```

## Two transports

Both are the same exchange: `POST`, `application/json`, the correlator only when there
is one, `Retry-After` in both of RFC 9110 §10.2.3's forms, and the same
`TransportError` case for the same situation — the two suites in `src/transport.rs`
mirror each other test for test, which is how that is checked rather than asserted. What
differs is the stack beneath, and the stack is the whole reason there are two.

`HttpTransport` is reqwest 0.13, and reqwest 0.13's `rustls` feature — and its
`rustls-no-provider` feature too — depends on **rustls-platform-verifier**
unconditionally. That is the right default almost everywhere: it reads the *system* trust
store, so an internal CA the machine already trusts is trusted here. It is also not
something a mobile app can carry, and not something a consumer can switch off, because a
feature cannot remove a dependency.

Measured on 2026-10-04 with `cargo tree -p nlipsy-http-client -e normal,build` for each
target — which needs neither target installed, nor an NDK, nor Xcode, and is what
`make check-mobile-graph` runs on every CI build:

| target | `--features reqwest` (default) | `--no-default-features --features hyper` |
|---|---|---|
| `aarch64-linux-android` | 112 crates, including `jni`, `jni-sys`, `rustls-platform-verifier`, `rustls-platform-verifier-android`, `aws-lc-rs`, `aws-lc-sys`, `cmake` | **58 crates**; `ring`, `webpki-roots`, `cc` and nothing else of the above |
| `aarch64-apple-ios` | 103 crates, including `rustls-platform-verifier`, `security-framework`, `aws-lc-rs`, `aws-lc-sys`, `cmake` | **58 crates**; the same three |

`jni` is the half that hurts most: it needs a Kotlin companion library shipped and
initialised in the app, which is a second artefact and a second lifecycle for something
the app only wanted in order to make one `POST`. `aws-lc-sys` is a vendored BoringSSL
built by `cmake`, so it is a C toolchain in the build of every target.

**What `HyperTransport` gives up.** The roots are Mozilla's bundle from `webpki-roots`,
frozen at build time, plus whatever the application adds itself. So:

- an enterprise or internal CA the device trusts is **not** trusted here because the
  device trusts it. The application can hand it over — DER, one call per root —
  through `HyperTransportBuilder::add_root_certificate`, which *adds* to Mozilla's
  bundle and never replaces it; but the device's own store is still never read, so a CA
  the machine was told about stays unknown here until the application is told too;
- the platform's proxy settings are not consulted. A consumer with a route of its own
  expresses it as a `Dialer` instead — see below;
- there is no certificate pinning, and no revocation data beyond what the certificates
  carry;
- a root Mozilla adds or removes reaches a deployment only when it is rebuilt against a
  newer `webpki-roots`.

It is also not a runtime: hyper needs Tokio and this crate starts none. A
`HyperTransport` is built and used inside whatever reactor the consumer already has.

**Which to pick.** A server, a daemon, a CLI, a container, anything that can ask the
platform: `HttpTransport`. An Android or an iOS app over FFI, or any build where a C
toolchain in the dependency graph is the problem: `HyperTransport`, with
`default-features = false, features = ["hyper"]` so that reqwest is not compiled at all.
A deployment that needs the *platform's* store on a mobile target has a third option
that is better than either: hand `nlipsy_core::Transport` the platform's own HTTP stack —
`URLSession`, `OkHttp` — as an implementation of its own. It is one `async fn`, and that
is what the seam is for.

Through the facade the same choice is `nlipsy = { features = ["http-client"] }` and
`nlipsy = { features = ["http-client-hyper"] }`; neither implies the other, so the second
one compiles no reqwest either.

### Routing the connection yourself

By default `HyperTransport` resolves the end-point's name with the system resolver and
opens a TCP socket itself, through hyper-util's `HttpConnector` — exactly as it did
before this seam existed. A consumer that owns the route cannot use either: an app with
a userspace WireGuard tunnel inside it, whose names are answered by the tunnel's own DNS
server and whose packets must never touch a system socket. `HyperTransportBuilder::dialer`
takes an `Arc<dyn Dialer>`, and from then on the transport resolves **no** name and opens
**no** socket: each new connection is one call to `Dialer::dial` with the URI's host, as
written, and its port, and TLS, HTTP/1.1 and the pool run over the Tokio stream that
comes back.

What an implementation can rely on and what it owes is written out on the trait, and the
short version is:

- **one `dial` per new connection, reuse only through the pool, and the pool is the
  transport's own** — a consumer whose route changes builds a new transport, and the old
  route's connections go with the old one;
- **the future may be dropped at any time** — the connect budget expiring, a caller
  abandoning an exchange — so it must release what it holds on drop; and hyper-util may
  let a started dial finish in the background when a pooled connection wins the race,
  pooling the result, so an extra dial is possible (never one shared across transports);
- **the name is verified, not the route**: SNI and certificate verification use the
  URI's host, whatever address the dialer actually reached;
- **the connect budget bounds the whole `dial`**, the dialer's own DNS included — unlike
  the default path, which resolves first with no budget and then splits it across the
  addresses. The TLS handshake is inside neither;
- **a failed or timed-out dial is `TransportError::Unreachable`**, with the `io::Error`
  and its `source` chain in the detail, word for word.

`Dialer` is a trait object and `HyperTransport` stays one concrete type: inside, its
connector is an enum of `HttpConnector` and the dialer, so a build that never names a
dialer runs the very same connector it always did, unboxed.

## What this crate decides, and what it refuses to

**No request timeout.** The connect phase is bounded — a connection that was never
established cannot have left a turn anywhere, so giving up on it is safe and
reportable — and the exchange as a whole is not. A model-backed end-point may
legitimately think for minutes, a caller cannot learn its budget, and abandoning a
request the end-point is still working on would leave the turn recorded on one side and
not the other. That divergence is invisible on the wire, which is why waiting is the
caller's decision and the caller's own limit goes around the call.

**`Unreachable` and `Interrupted` are kept apart.** reqwest knows whether the
connection was ever established, `hyper_util::client::legacy::Error::is_connect` is the
same answer on the other transport, and that is the distinction a retry policy turns on:
the first can be repeated freely, the second may already sit in the end-point's
transcript. Flattening them into one error would be a retry that duplicates a turn.

**A reply that is not a message still reaches the caller.** Over HTTP the end-point sits
behind intermediaries that answer in their own voice — a body limit refused in plain
text, a proxy's HTML, a route that did not match — and none of those bytes came from an
NLIP implementation. They arrive as `nlipsy_core::Body::Opaque` with the reason, because
the text is what the user has to see.

**`X-NLIP-Correlator` is sent only when there is one.** An empty header would be a claim
that there is a turn to correlate with when the caller never named one.

**`Retry-After` is read in both of RFC 9110 §10.2.3's forms.** A count and an HTTP-date
are both legal and an end-point behind a cache often sends the date, so a client that
read only the count would turn a paced retry into a blind one. The rule is
`nlipsy-http`'s — `parse_retry_after(value, now)`, with no clock of its own — and the
clock is this crate's, which is the split that keeps the rule testable against a fixed
date.

## What this crate no longer witnesses

The crate this one grew out of lived in the reference server's repository and declared
`NLIP_PATH` and `CORRELATOR_HEADER` itself, and said why: "Declared here rather than
imported from the server: the two are separate implementations of the same clause, and
a two-token constant that both sides must agree on fails loudly when it drifts, not
silently". Client and server were two codebases, and agreement between them was
evidence.

Merging both sides into one workspace removed that witness, and the constants have since
become one line of code: `NLIP_PATH`, `CORRELATOR_HEADER` and `CONVERSATION_SUBFORMAT`
live in `nlipsy-http` and are re-exported from here and from `nlipsy-server`. Two halves
that agree because they are the same line of code agree about nothing — which is why the
replacement below is not optional.

The replacement is evidence from outside this crate's own encoder, and there are two
pieces of it:

- **The conformance corpus** under `conformance/` in this repository: recorded messages
  and the outcome every implementation must produce for them, as data rather than as
  Rust. Nothing in it was written by this crate, and a second implementation — the
  JavaScript package that is meant to follow — runs the same cases.
- **The recorded fixture** at `tests/fixtures/recorded-wire.jsonl`: two exchanges a live
  end-point really sent, request halves included, matched on replay. A fixture composed
  from this crate's own encoder would agree with a systematic misreading of ECMA-430 §5
  exactly as readily as with a correct one; these bytes cannot.

Neither replaces a second implementation of ECMA-431 §6 that this workspace does not
control. That is worth saying plainly rather than discovering later.

## Dependencies, and the ones that are a choice

Always: `nlipsy-core`, `nlipsy-http`, `nlipsy-protocol`, `thiserror`. Nothing with a
socket in it, which is what a build with neither transport feature compiles.

With `reqwest`: `reqwest`, with `default-features = false` and `rustls`. That is the
deployment-visible line — verification goes through rustls-platform-verifier, so a
container image carrying this client needs `ca-certificates` installed, and an internal
CA that the machine already trusts is trusted here too. reqwest's `json` feature is
deliberately off: `nlipsy-core` encodes the request and the reply comes back as bytes.

With `hyper`: `hyper`, `hyper-util`, `hyper-rustls`, `rustls`, `webpki-roots`,
`http-body-util`, `tokio`, `tower-service`. `default-features = false` on every one of
them and `aws-lc-rs` on none, which is the whole of the table above; the root
`Cargo.toml` says why each carries the features it does. `tower-service` is the one that
reads like an addition and is not: the transport's connector — `HttpConnector` or a
caller's `Dialer` — has to *implement* `Service<Uri>`, which neither hyper-util nor
hyper-rustls re-exports, and both of them already depend on that exact crate, so the
graph `make check-mobile-graph` counts is the same with it as without. The transport builds its `ClientConfig` through
`ClientConfig::builder_with_provider` and never `ClientConfig::builder`, because a build
that takes both features has a `rustls` carrying two crypto providers and the implicit
builder panics on exactly that.

`wasm32-unknown-unknown` is out of scope for this crate, unlike `nlipsy-protocol` and
`nlipsy-core`, which are checked for it on every build — and the second transport is not
an escape from that: hyper over tokio is a socket and a reactor, and a browser has
neither. A browser has `fetch`; a JavaScript package over the same core calls that
instead of compiling either of these.

## Where this crate sits

Six published crates, in the order they depend on each other:

| crate | is |
|---|---|
| [`nlipsy-protocol`](https://crates.io/crates/nlipsy-protocol) | ECMA-430 clause 5 — what a message *is* |
| [`nlipsy-core`](https://crates.io/crates/nlipsy-core) | ECMA-430 clause 6 — what an end-point *owes* a peer, and the sequence a binding drives, with no runtime in it |
| [`nlipsy-http`](https://crates.io/crates/nlipsy-http) | ECMA-431 §6 as values — the status map, the header spellings, `Retry-After` both ways |
| [`nlipsy-http-client`](https://crates.io/crates/nlipsy-http-client) | ECMA-431 from a client agent's end — one `POST`, over reqwest or over hyper |
| [`nlipsy-server`](https://crates.io/crates/nlipsy-server) | ECMA-431 from a server agent's end — one `axum::Router`, clause 6 underneath |
| [`nlipsy`](https://crates.io/crates/nlipsy) | the facade: all of them as modules behind feature flags |

Depending on any of them directly is supported and always will be. The facade is for
the consumer's manifest, not for their imports: `nlipsy = { version = "0.1", features =
["http-client"] }` is this crate, under the name `nlipsy::http_client`.

## Minimum supported Rust version

1.85, the edition 2024 floor — the same as the rest of the workspace, and reqwest
0.13's own. The `hyper` transport does not raise it: hyper-rustls declares 1.85 and
every other crate in that graph is below it (hyper 1.63, hyper-util 1.64, rustls 1.71,
webpki-roots 1.70, ring 1.66, http-body-util 1.61). Raising it is treated as a breaking
change, and it is checked rather than asserted: `cargo +1.85 check --workspace
--all-targets --all-features` is part of what a release runs, with
`cargo +1.85 check -p nlipsy-http-client --no-default-features --features hyper` beside
it for the build `--all-features` cannot reach.

`--all-targets` compiles the test suites too, so the dev-dependencies are held to the
same floor. That is why the dialer suite's test CA and leaf are checked in as DER under
`tests/fixtures/tls/`, with the OpenSSL recipe that regenerates them beside them, rather
than minted per run: `rcgen`, the generator that was tried, brings `time`, whose current
releases need 1.88, and three files of bytes have no rust-version.

## Licence

Licensed under either of

- Apache License, Version 2.0
  ([LICENSE-APACHE](https://github.com/laktoosivaba/nlipsy/blob/master/LICENSE-APACHE)
  or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence
  ([LICENSE-MIT](https://github.com/laktoosivaba/nlipsy/blob/master/LICENSE-MIT)
  or <https://opensource.org/licenses/MIT>)

at your option.

The links are spelled as full URLs rather than as `[LICENSE](LICENSE)`: this file is the
crate documentation, and rustdoc reads a relative link with no extension as an intra-doc
link to a Rust item — one that does not exist, which is a warning on every build.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 licence, shall be
dual-licensed as above, without any additional terms or conditions.

ECMA-430, ECMA-431, ECMA-432, ECMA-433, ECMA-434 and TR/113 are publications of Ecma
International and are not redistributed here.
