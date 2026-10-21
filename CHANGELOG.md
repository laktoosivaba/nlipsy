# Changelog

All notable changes to this workspace are recorded here. The format is based on
[Keep a Changelog], and the workspace follows [Semantic Versioning].

Every crate in the workspace shares one version, declared in `[workspace.package]`,
so one section covers the release as a whole.

While the version is 0.x, a raise of the minimum supported Rust version is treated
as a breaking change and gets its own minor bump.

[Keep a Changelog]: https://keepachangelog.com/en/1.1.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html

## [Unreleased]

Nothing yet.

## [0.1.2] — 2026-10-21

The first release, and it is six crates rather than one. NLIP is a message model with
several transports over it, so the Rust for it is a message model crate, a runtime core
crate holding the obligations every transport shares, a crate per *standard* holding
what both ends of that standard's binding must agree on, one crate per binding, and a
facade so that a consumer names one dependency and not five. Nothing below has been
published to crates.io before; the sections are ordered as the crates depend on each
other, which is also the order they are published in.

**Why 0.1.2 and not 0.1.0.** 0.1.0 and 0.1.1 were test publishes to a private
registry, on 2026-10-03 and 2026-10-04, and a registry's versions are immutable: a
version with different contents would break the checksum of every consumer that had
already fetched the first one. The hyper transport (`nlipsy-http-client`, "Two
transports") and the facade's `http-client-hyper` feature landed after the first of
those publishes, and `nlipsy-server`'s `ServerConfig::principal_header` and
`HyperTransport`'s pluggable `Dialer` and `add_root_certificate` after the second, so
the version moved each time. This section describes the crates as they are at 0.1.2,
the first version intended for crates.io.

Everything here is `### Added`, since there is nothing to change or remove yet.

### `nlipsy-protocol` — the message model, ECMA-430 clause 5

- **`Message`, `Submessage` and `Format`** — the latter exhaustive over §5.3 Table 1,
  with `FromStr`. A message is a first submessage (`format`, `subformat`, `content`,
  optional `messagetype`) followed by an ordered list of further submessages; order is
  significant (§5.1.5) and preserved.
- **`Content` and `Wire`, in module `content`.** `Content` is the ECMA-404 value
  space — `Null`, `Bool`, `Number`, `String`, `Array`, `Object` — plus `Bytes`, which
  JSON has no room for. `Wire` (`Json`, `Cbor`) names which encoding a value arrived
  on, because that is what decides how binary content is spelled.
- **One decoder.** `Message::from_content(Content, Wire)` holds every rule;
  `from_json_bytes`, `from_json_str` and `from_value` funnel into it, so the three
  entry points cannot drift apart.
- **Binary content on both wires.** Base64 on the JSON wire, decoded to real bytes
  (`ParseError::Base64`); a raw byte string on CBOR. Base64 is treated as JSON's
  spelling of bytes rather than as the content itself.
- **`cbor` feature**, off by default: `Message::from_cbor_slice` and
  `Message::to_cbor_vec` over ciborium (ECMA-432 §7.1), plus `ParseError::Cbor` and
  `ParseError::TrailingBytes`. CBOR tags, non-text map keys and non-finite floats
  (NaN, ±∞) are refused — none of them has a meaning in the NLIP value space, and
  delivering a guess would hand the application a value the peer did not send. A
  frame carrying more than one message is `TrailingBytes`: §7.1 allows "a single
  NLIP message", and the JSON wire already refuses trailing characters.
- **serde `Serialize` and `Deserialize`** for `Message`, `Submessage` and `Content`,
  for callers embedding a message in a larger document. `Content::Bytes` follows the
  format's `is_human_readable`: bytes to a binary format, base64 to a textual one.
- **Case-insensitive field handling with duplicate detection.** §5 —
  "Capitalization is not relevant to the name or value of the field." Names and
  format values are matched case-insensitively and re-encoded in the canonical
  lower-case spelling. The same field twice under different casing is an error, not
  a silent choice between the two.
- **Typed `ParseError` carrying the path to the failure**, so a caller is told which
  submessage and which field was malformed instead of a position in a byte stream.
- **`validate`, returning `Violation`s with clause-numbered rule ids** — including
  `ecma432-7.1/binary-content-text` and `ecma430-5.3/binary-encoding-parameter`. A
  message that parsed but breaks a rule is a separate outcome from one that could
  not be read, and the id names the clause so a deployment can decide per rule.
  `content: null` is a value, not an omission (Annex A), and is reported here.
  Two severities follow the clauses rather than the convenient reading:
  `ecma430-5.3/binary-content` — content under `binary` that is neither bytes nor
  text — is an `Error`, because §5.1.4 makes the same demand of every format and
  §6.4 already defines how a payload travels by reference; and a non-string `token`
  content is a `Warning` of its own, `ecma430-5.3/token-content`, because Table 1
  calls it an opaque token without fixing its type. `ecma430-5.1.4/text-content`
  stays an `Error` and applies to `text` only.
- **Token classification on both levels.** `Submessage::is_token`,
  `is_authentication_token` and `is_conversation_token`, and the same three on
  `Message` for the first submessage — which `Message::tokens` cannot yield, because
  it is not a `Submessage` value. §6.2 ¶2 obliges an end-point to echo every token
  submessage a message contains, and a message may be one.
- **`Message::upload_request` and `Message::upload_response`** — ECMA-430 §6.4's two
  messages, in the literal shape of ¶2 and ¶3. The request is a `control` message
  whose `text` / `english` first submessage asks for the end-point; the response is a
  `control` message whose first submessage says what the address is for and whose
  `submessages` carry it as a `Submessage::uri`. Both write `MessageType: control` and
  neither sets the boolean `control` field: §6.4 names `MessageType` and only that,
  while §6.3's boolean belongs to the end-point that owes §6.3. `Message::uri` is
  unchanged and its doc says it is *not* the §6.4 answer — it puts the URI in the
  first submessage, where §5.1.4 puts the message's own content, rather than in the
  submessage ¶3 asks for.
- **Other builders and accessors**: `Message::binary`, `Message::error`,
  `Submessage::uri` — the `structured`/`uri` submessage §6.4 ¶3 asks a control response
  to carry — `Submessage::text`, `content_as_bytes`, `extract_text`.
- **`ParseErrorKind` and `ParseError::kind`** — what a document that could not be
  decoded at all was wrong about, in the nine-value vocabulary the conformance corpus
  fixes (`conformance/README.md`, "Expectations"). It is not one case per `ParseError`
  variant and deliberately: the corpus names what an implementation must *distinguish*,
  so ECMA-432 §7.1's "a frame carries one message and no more" is a `Cbor` failure here
  rather than a kind of its own. The mapping used to live in the corpus runner, with a
  catch-all answering `Unmapped` for a variant it did not know; it is here now because
  it is not the runner's opinion but what this implementation claims to have
  distinguished, and the JavaScript package's component has to report the same nine
  values for the same bytes. Two copies would be two answers to one question. Inside the
  crate the `match` is exhaustive, so a variant added later stops it compiling instead of
  landing silently on a neighbour; `ParseErrorKind::Unmapped` is what an implementation
  with no such compiler check answers, and `as_str` is the corpus's kebab-case spelling.
- **`#[non_exhaustive]` on `Message`, `Submessage`, `ParseError` and `Violation`**,
  so a field or variant the standard already implies can be added without a breaking
  release. `ParseErrorKind` is deliberately *not* one, like `Severity` and `Format`: a
  consumer branching on all nine should keep being told when it has missed one.

### `nlipsy-core` — the runtime core, ECMA-430 clause 6

- **Clause 6 as values and traits, with no opinion about execution.** `Conformity` and
  its synchronous steps — `check` before the handler, `finish` after it, `echo_into` on
  the failure path, `close` over a reply `finish` never saw — the `Endpoint` and
  `TokenAuthority` seams behind
  feature `server`, the `Transport`, `Client`, `Reply` and record/replay machinery
  behind feature `client`, and `Context` — `peer`, `correlator`, `http::Extensions` —
  shared by both. No default features, as `jsonrpsee-core` has none, and the halves are
  gated the way it gates them.
- **No runtime in it, as a checked property.** The dependency list is
  `nlipsy-protocol`, `thiserror` and `http` (for `Extensions`, re-exported), plus
  `serde` and `serde_json` under `client` for the recorded-exchange format: no
  executor, no socket, no clock and no file I/O. So the crate compiles for
  `wasm32-unknown-unknown` and every clause-6 rule is testable without an executor,
  which is the test that decides what belongs in it at all. Traits are bounded by a
  `MaybeSend` marker — `Send` on a target with threads, nothing on `wasm32` — copied
  from jsonrpsee for the same reason it exists there: a JavaScript callback is neither.
- **Three things the extraction settled that the reference server had written in
  code.** The token ceiling is `ConformityConfig::max_tokens_per_message` rather than a
  `const`. What a clause-5 finding *does* is a `Rejection` value with a separate policy
  per direction — an end-point that tolerates a peer's warnings should not therefore
  tolerate its own. And §6.2.1's "keep confirming the token you created" is
  `TokenAuthority::issued`, a read with no side effect asked on every reply, separate
  from `TokenAuthority::mint`, the write asked once. One method could not be both: a
  peer that returns this end-point's conversation token *beside* one of its own has the
  first withheld by §6.2's exception and the second blocking a mint, and the identifier
  the end-point owes back would fall between them. Every default reproduces the
  previous behaviour exactly.
- **`Conformity::close`, and what it makes true.** `close(request, &mut reply, cx)` is
  §6.2's echo, §6.2.1's confirmation of an already-issued token and §6.3's carry — the
  three obligations that are idempotent — applied again over a reply that may not have
  been through `finish`. A `Middleware` can answer `Ok` without calling `Next::run`, and
  a layer's after-half holds the finished reply by value and can strip a token out of
  it; both are ordinary things to write, so a binding closes clause 6 afterwards rather
  than assuming a constructor settled it. It never mints: minting is a write, and the
  paths that need `close` are the ones a deployment does not commit. `finish` is
  unchanged in effect and remains the only place a first token is created.
- **§6.3 is applied as the unconditional clause it is.** A handler that answers a
  control request with `"control": false` is overruled — the boolean is overwritten,
  not filled — because the clause carries no exception and "the end-point" is the whole
  stack from the peer's side. `messagetype` is still only filled where the handler left
  it empty: §6.3 names the boolean and no other field.
- **A reply replaced by the clause-5 sanitiser keeps `MessageType: Error`** and carries
  control-ness by §6.3's boolean alone, where it used to be rewritten to `Control`.
  §5.1.1 gives the field one value, and `Error` is the one in-band marker a peer has
  for a failure — including this workspace's own client, whose `Reply::is_error` reads
  it. `Finished::withheld()` reports that the replacement happened, so a binding can
  say so on its status line.
- **`serve` and `serve_message`, the composer a binding drives** (feature `server`).
  `Conformity` holds the clause-6 obligations; this holds the **order they run in**,
  which is the other half of being an end-point and had until now been written out
  inside `nlipsy-server`: decode, `check` *outside* the layer stack, the deployment's
  layers with `finish` at the **bottom** of them, `close` over whatever came back
  **unconditionally**, and the failure rules instead on any failure. It returns
  `Served { reply, outcome, withheld, findings }` and never an error — every way an
  exchange can end produces a message the peer can read, because a binding that has to
  invent a body for a failure will invent a different one from the next binding. The two
  emphasised words are what a reply that a layer answered in the handler's place still
  owes — the echo, the confirmation and the control flag, but never a fresh mint — and
  they are the reason this is code rather than a paragraph in a README: a second binding
  — ECMA-432's frames, a `fetch` handler in JavaScript over the same engine — would
  otherwise re-derive the order and get one of them wrong. What stays in the binding is
  everything the order has no opinion about: reading the bytes, the status line, the
  headers, the log and the clock. `Conformity`'s own `Endpoint` implementation is
  unchanged and remains the one-`await` convenience.
- **`Outcome`, and the failure rules as functions over values** (feature `server`).
  `Outcome` is the four ways an exchange ends — `Ok`, `Malformed(ParseError)`,
  `Refused(Rejected)`, `Failed(EndpointError)` — with `public_message` (the text the
  peer may see) and `retry_after` (the delay a dependency named, as a `Duration`; the
  header is a binding's spelling). `owed` says what clause 6 still owes on each —
  nothing when the bytes never decoded, §6.3 alone when the token ceiling refused,
  everything `Conformity::close` discharges otherwise — and `error_reply` builds the
  reply, for the composer and for a binding's own refusals alike (a rate limiter that
  has already decoded the request owes the peer the same three obligations). Both are
  synchronous and tested without an executor.
- **`Middleware`, `DynMiddleware` and `Next`** (feature `server`) — the message-level
  layer seam: `call(&self, request: &Message, cx: &mut Context, next: Next<'_>)`,
  stacked outermost-first as a list of trait objects and run between `Conformity::check`
  and the endpoint. `&mut Context` is the whole point — §6.2's exception is answered by
  a `TokenAuthority` that reads the deployment's session out of `Context::extensions`,
  so something has to put it there after decoding and before the endpoint runs, which is
  where jsonrpsee inserts its own `ConnectionId`. Not `tower::Service`, for jsonrpsee's
  reasons: `poll_ready` has nothing to gate with one handler at the bottom, `call` takes
  `&mut self` where a shared endpoint wants `&self`, and a domain trait can lend a layer
  the request and the context instead of moving them into a future — which §6.2's echo
  needs, since it reads the request while writing the reply. It is in the core rather
  than in a binding because it needs nothing a binding has: a JavaScript package serving
  NLIP over its own HTTP framework wants the same seam.
- **`Dispatcher`** (feature `server`) — §5.1.1's two-way split, control messages to one
  endpoint and everything else to another. Protocol-generic and runtime-free, and it is
  the whole of NLIP's routing: there is no method table to build, because a message
  names no method. Its `Endpoint` implementation is not compiled for `wasm32`, which is
  the same asymmetry `impl Endpoint for Arc<T>` spells as a `where` clause: `Endpoint`
  requires `Sync`, `Arc<T>: Sync` requires `T: Send`, and `MaybeSend` is deliberately
  not `Send` there. The routing rule itself — `Dispatcher::route` — is available on
  every target.
- **`EndpointError`** with `Unsupported`, `Unavailable { retry_after }`, `Internal`,
  and two variants the extraction added. `TimedOut { after: Duration }` is a `Duration`
  and nothing else — the clock stays outside the core, and producing this variant is the
  business of whatever holds the timer; it is separate from `Unavailable` because a
  binding answers the two differently, a dependency that named a delay being a `503`
  with the delay attached while an endpoint that ran out of time is a `504` with nothing
  to promise. `Refused(Rejected)` is what lets `Conformity`'s own `Endpoint`
  implementation return a clause-6 refusal through the one error type its callers
  already match on.
- **`Client` carries the wire.** `Client::new` is unchanged in signature and in
  meaning — ECMA-431's JSON — and `Client::on_wire` is ECMA-432 §7.1's CBOR, in the
  builder spelling `Replay::on_wire` already uses; `send` encodes and decodes on the one
  setting, and `wire()` reports it. It is here because the alternative was a second copy:
  a binding that had to choose per call wrote `send` out again around `encode_request`
  and `decode_reply`, which is what the WebAssembly guest of `wit/nlipsy.wit` did.
- **`Reply::tokens`, and the two string views derived from it.** §6.2 ¶2 obliges an
  end-point that received a token submessage to include "the same submessage with the
  exact same format, subformat and content" in its response, and the clause is
  symmetric: a client is an end-point and its next request is its response. `tokens`
  is that list — the first submessage first (`first_submessage_token`, so a reply that
  *is* a token is not lost), then the `submessages` array in document order, with the
  label and a content that is not a string carried verbatim. The clause's exception is
  deliberately not applied here: removing the tokens the caller minted itself needs the
  caller's memory of what it minted, which no crate on this side of the wire has.
  `Reply::conversation_tokens` and the new `Reply::authentication_tokens` are derived
  from it, so the three cannot disagree and a token-only reply now reaches both;
  they are keys — for a session table, for a status line — and a client that rebuilt a
  submessage from one of those strings would choose a subformat and drop the label,
  which is exactly what "the exact same" forbids. §6.2.2 is the mandatory half of
  clause 6 and had no view at all until now.
- **Record and replay over an abstract sink.** `Recorder<T>` writes to a `RecordSink`
  trait rather than to a file, so the core carries no file I/O; `Replay::from_jsonl`
  parses a recorded exchange out of text it is handed. The file-backed halves —
  `FileSink` and `Replay::load` — live in `nlipsy-http-client`, which is the crate
  allowed to know what a path is. `Replay` matches the *request* as a decoded `Message`
  and returns the recorded *reply bytes* verbatim, so a fixture still bypasses this
  workspace's own encoder on the way back.
- **`Scripted`**, behind feature `testing`, for the tests of other crates. It cannot be
  `#[cfg(test)]`, which would compile it only for this crate's own unit tests, and a
  release build of any consumer compiles no part of it.
- **`serve_answered`, and `serve_message` re-expressed as it.** The same order —
  `check`, the answer, `close` over whatever came back, and the failure rules on every
  error — with the middle supplied by the caller as a future rather than composed here.
  It exists because a binding can own its own layer stack: a caller that composed its
  own layers and called `Conformity::finish` at the bottom of them holds a `Finished`
  rather than a bare `Message`. Handing that to `serve_message` would run
  `Conformity::finish` a second time over it — and §6.2.1's minting is in `finish`, so
  an authority that had not yet been told about the first mint would create a second
  identifier and the peer would be handed both. `close` is the idempotent half and is
  what such a reply still owes. `serve_message` *is* `serve_answered` with the layer
  stack as its answer, so the order is written once and both callers get it.
  `Conformity::finish` is the only producer of a `Finished`, so there is no way to
  present a reply that skipped the step.

### `nlipsy-http` — ECMA-431 §6 as values

- **The clause both ends of the HTTP binding have to agree on, in one place.** §6 fixes
  the path and the method and nothing else, so which status a refusal carries, what
  `Retry-After` means in each direction, what body a `405` or a `413` has, and what the
  correlation header is called are all *choices* — and a choice made twice is a choice
  made differently. `StatusMap` (with `of(&Outcome, withheld)` and
  `of_error(&EndpointError)`, defaults the reference server's codes), `NLIP_PATH`,
  `CORRELATOR_HEADER` and `correlator_header()`, `CONTENT_TYPE`,
  `DEFAULT_MAX_REQUEST_BODY_SIZE`, `CONVERSATION_SUBFORMAT`, `retry_after_value`,
  `parse_retry_after`, `request`, `method_not_allowed` and `too_large`.
- **No socket, no framework, no executor and no clock**, and `make check-wasm` compiles
  it on every run. That is the other half of why it exists now rather than later: the
  engine is being packaged for JavaScript over the Component Model, and a `fetch`
  handler there needs exactly these values. As functions over values they cross the wasm
  boundary unchanged; inside an axum binding they would have had to be written a second
  time, in another language, from the standard's prose.
- **`parse_retry_after(value, now)` takes the instant as an argument.** RFC 9110
  §10.2.3 allows a count *or* an HTTP-date, so a client must read both — and the
  previous reader in `nlipsy-http-client` read only the count. A date already past yields `Some(Duration::ZERO)`
  rather than `None`: the end-point answered the question and the answer is "now", which
  a caller has to be able to tell from "it said nothing I can use".
- **`httpdate`** is the one new dependency in the workspace — `MIT OR Apache-2.0`, no
  dependencies of its own, no clock, already in the lockfile through hyper. It is the
  IMF-fixdate parser RFC 9110 §10.2.3 needs; a calendar library would have brought a
  time-zone database for a header whose grammar fixes GMT.
- **The two constants that used to be declared twice on purpose are declared once.**
  `nlipsy-server` and `nlipsy-http-client` each spelled `/nlip` and `x-nlip-correlator`
  themselves, with the duplication argued for: "the two are separate implementations of
  the same clause, and a constant both sides must agree on fails loudly when it drifts
  rather than silently". That was evidence while the two lived in different
  repositories. In one workspace it was a second copy to keep in step, and the
  conformance corpus and the recorded fixtures are the witness instead.

### `nlipsy-http-client` — the ECMA-431 client binding

- **One `POST` to `https://<server_name>:port/nlip`** carrying one JSON message and
  returning one, which is the whole of what §6 asks of a client agent: `HttpTransport`
  implements `nlipsy_core::Transport` over reqwest, `HttpTransportBuilder` carries the
  connect budget and any headers a deployment adds, `NLIP_PATH` and `CORRELATOR_HEADER`
  are the binding's two constants, `request` is the TR/113 §11.1 chat turn, and
  `FileSink` and `Replay` are the file-backed half of the core's recorder. The
  correlator is sent only when `Context::correlator` holds one — an empty header would
  claim a turn to correlate with where the caller named none — and `Retry-After` is read
  into `ExchangeDetail::Http`.
- **No request timeout, deliberately.** A model-backed end-point may think for minutes,
  and abandoning a request it is still working on leaves the turn recorded on one side
  only. The connect phase *is* bounded, because a connection never established cannot
  have left a turn anywhere.
- **rustls, and the system trust store.** reqwest is taken with
  `default-features = false` and `rustls`, so verification goes through
  rustls-platform-verifier; an image carrying this client needs `ca-certificates`.
  `cbor` forwards to `nlipsy-core/cbor`.
- **Two transports, behind two features, and the second one is a dependency graph
  rather than a preference.** `HyperTransport` is the same exchange over hyper +
  hyper-rustls on rustls's `ring` provider, with Mozilla's roots compiled in from
  `webpki-roots`, behind a `hyper` feature; `HttpTransport` is unchanged behind a
  `reqwest` feature, which is the default, so nothing a consumer already wrote changes.
  The crate also compiles with **neither** — `request`, `FileSink`, `Replay` and the
  re-exports, which is what a consumer who only replays a recording should have to
  build.

  The reason is that the crate is going into an Android and an iOS app over FFI, and
  reqwest 0.13's `rustls` feature — and its `rustls-no-provider` too — depends on
  rustls-platform-verifier *unconditionally*: on `aarch64-linux-android` that is `jni`
  plus a Kotlin companion library the app has to ship and initialise, and on every
  target it is aws-lc-sys, a vendored BoringSSL built by cmake. A feature cannot remove
  a dependency, so a second transport was the only way to offer a build without it.
  Measured with `cargo tree -e normal,build` on 2026-10-04: 112 crates for
  `aarch64-linux-android` and 103 for `aarch64-apple-ios` by the default transport,
  against **58 for either target** by the hyper one, whose TLS is `ring`,
  `webpki-roots` and `cc` and nothing else. `make check-mobile-graph` is that as a gate
  — it reads the tree back for both targets on any machine with cargo, needing no NDK
  and no Xcode, and fails on any of `jni`, `rustls-platform-verifier`, `aws-lc`,
  `cmake`, `security-framework`, `openssl`, `native-tls` — and it runs in CI's feature
  job, because the property is the whole point of the transport and nothing in the
  source can state it.

  What the second transport **gives up** is the system trust store, and that is said in
  its own documentation rather than left to be discovered: no enterprise or internal CA
  however the device is configured, no platform proxy settings, no pinning, and a root
  bundle frozen until the next build. A deployment that needs the platform's store on a
  mobile target has a better third option than either transport — hand
  `nlipsy_core::Transport` the platform's own HTTP stack, `URLSession` or `OkHttp`, as
  an implementation of its own. It is one `async fn`, and that is what the seam is for.
- **The contract is one contract, and that is checked rather than asserted.** Both
  transports are `POST` with `application/json`, both send the correlator only when
  `Context::correlator` holds one, both read `Retry-After` in both of RFC 9110
  §10.2.3's forms *through one reader*, and both report the same `TransportError` case
  for the same situation — `Unreachable` when the connection was never made
  (`hyper_util::client::legacy::Error::is_connect` is hyper's answer to reqwest's
  `is_connect`), `Interrupted` otherwise, and `Interrupted` with the sentence "reply
  headers arrived but the body did not" word for word when the head arrived and the
  body did not. The two suites in `src/transport.rs` mirror each other test for test
  over one hand-written HTTP/1.1 responder, which is where that agreement is written
  down.
- **Three deliberate differences, all of them in the hyper transport's favour.** The
  end-point is parsed into a `hyper::Uri` once in `build()`, so a string that is not a
  URI is a `BuildError` handed to the caller where they wrote it rather than a failed
  exchange per request; a request that could not be *formed* at all (a correlator that
  is not a legal header value) is `Unreachable`, which is what `TransportError`'s own
  documentation says that case means, where reqwest's error model happens to report
  `Interrupted`; and the connect budget is applied with
  `HttpConnector::set_connect_timeout`, because hyper-util's pooling client has no
  timeout of its own and a `tokio::time::timeout` around the exchange would bound the
  end-point's thinking time — the one thing this crate refuses to do.
- **`ClientConfig::builder_with_provider`, never `ClientConfig::builder`.** A build that
  takes both features has one `rustls` carrying both the `ring` and the `aws-lc-rs`
  providers, because feature unification is additive and reqwest's verifier asks for the
  second; the implicit builder panics at run time on exactly that. Naming the provider
  is what makes the transport independent of what else is in the binary, and the line
  says so.
- **A pluggable dialer for `HyperTransport`.** `HyperTransportBuilder::dialer` takes an
  `Arc<dyn Dialer>`, and the transport then resolves no name and opens no socket itself:
  each new connection is one `Dialer::dial(DialTarget { host, port })` — the URI's host
  as written, IPv6 without brackets, and the explicit or scheme-default port — returning
  a boxed Tokio `AsyncRead + AsyncWrite` stream (`DialFuture`, `DialStream`,
  `DialedStream`; `DialTarget` is `#[non_exhaustive]`). TLS is unchanged over it: SNI and
  verification use the URI's host, not the route. `connect_timeout` bounds the whole
  `dial`, DNS included, and a failed or timed-out dial is `TransportError::Unreachable`
  with the `io::Error` and its source chain in the detail. Without `dialer()` nothing
  changes — the connector is an internal enum whose default arm is the same
  `HttpConnector`, unboxed, so `HyperTransport` stays one concrete type. Its `Debug`
  now names the dialer. One new direct dependency, `tower-service`, which was already in
  the graph under hyper-util and hyper-rustls; `make check-mobile-graph` counts the same
  58 crates.
- **Extra trust roots for `HyperTransport`.** `HyperTransportBuilder::add_root_certificate`
  takes one DER certificate per call and *adds* it to Mozilla's bundle, never replacing
  it — for an end-point behind a private CA, which the compiled-in bundle cannot verify.
  A DER that rustls cannot use as a trust anchor is a `BuildError` from `build()`, not a
  failure per exchange. No PEM variant: parsing PEM is the caller's.
- **The Tokio runtime is the caller's.** The transport starts none and spawns nothing
  beyond hyper-util's connection tasks, so a `HyperTransport` is built and used inside
  whatever reactor the consumer already has — single-threaded FFI hosts included. The
  `hyper` feature is also not an escape from the wasm exclusion above: hyper over tokio
  is a socket and a reactor, and a browser has neither.
- **Excluded from `make check-wasm` and from CI's wasm32 job**, and the exclusion states
  the wasm promise precisely rather than weakening it: reqwest on that target is the
  fetch backend, a different client with neither `connect_timeout` nor `is_connect`, and
  a JavaScript package reaches the core through `fetch` instead of compiling a native
  binding.
- **What moved to `nlipsy-http`.** `NLIP_PATH`, `CORRELATOR_HEADER`,
  `CONVERSATION_SUBFORMAT` and `request` are declared there now and **re-exported from
  here under the same paths**, so nothing a consumer wrote changes. `Retry-After` is
  read through `nlipsy_http::parse_retry_after(value, SystemTime::now())`, which
  understands the HTTP-date form as well as the count — the clock stays in this crate
  and the rule does not.
- **A loss, recorded.** The code this grew out of lived in the reference server's
  repository and declared `/nlip` and `X-NLIP-Correlator` *independently of the server*,
  so a drift between the two implementations of one clause failed loudly. In one
  workspace that independent witness is gone; the crate's README says so under "What
  this crate no longer witnesses", and names the replacements — the conformance corpus,
  and the recorded fixture `tests/fixtures/recorded-wire.jsonl`, two exchanges recorded
  from a live NLIP end-point, which moved here byte for byte with the code they test.

### `nlipsy-server` — the ECMA-431 server binding

- **An `axum::Router` for `POST /nlip` and `POST /nlip/`** with clause 6 underneath it:
  read the body under a limit, hand the bytes to `nlipsy_core::serve`, map the
  `Outcome` it reports onto a status line, attach `Retry-After`, echo the correlator.
- **The sequence and its failure rules are `nlipsy-core`'s.** The pipeline, the private
  `Owed` enum, the error-reply construction and the `Tail` that put `Conformity::finish`
  at the bottom of the layer stack all moved there, because none of them is HTTP and a
  second binding would otherwise re-derive the order. `StatusMap` and the `Retry-After`
  spelling moved to `nlipsy-http`, shared with the client binding that reads what this
  writes; `NLIP_PATH`, `CORRELATOR_HEADER`, `CONTENT_TYPE` and
  `DEFAULT_MAX_REQUEST_BODY_SIZE` moved there too. Every one of them is **re-exported
  under the same path**, so `nlipsy_server::StatusMap` and the rest keep resolving. What
  is left here is the socket's business: the router, the body limit, the correlator, the
  status line, the headers, the log and the §6.1 upload leg. The 37 tests of
  `tests/endpoint.rs` pass unchanged, which is what says the behaviour did not move with
  the code.
- **`StatusMap::of` now takes an `Outcome` and `withheld`** rather than a bare
  `EndpointError`, because the composer reports all four ways an exchange ends and a
  binding maps all four. The old mapping is `StatusMap::of_error`, which the upload leg
  still uses: that leg has no clause 6 around it and therefore no outcome.
- **`Server::new(endpoint, authority, config)` is the only way in.** There is no
  constructor that takes a bare `Endpoint` and no way to reach the router without one,
  so a deployment's layers stack outside `Conformity` and cannot displace it. That is
  `jsonrpsee-server`'s arrangement, whose innermost service is always its own
  `RpcService`, and it is here because the echo is the obligation a reply built from
  scratch silently drops. A constructor is not the whole of it: a layer can answer
  without ever reaching `Conformity::finish`, and can edit the finished reply on its way
  out, so the pipeline runs `Conformity::close` over whatever the stack returned.
  **Nothing a layer does can remove what clause 6 owes the peer**; a layer can still
  break clause 5 after `finish`, and the outbound policy does not see it.
- **Every refusal confirms what the endpoint issued, and one refuses to echo.** A `503`,
  `504` or `500` now carries the conversation token this endpoint is confirming as well
  as the peer's own — a client following `Reply::conversation_tokens`'s advice used to
  drop the server's identifier after one throttled turn. The exception is the token
  ceiling: a request above `ConformityConfig::max_tokens_per_message` is refused with no
  echo at all, because cloning what it refused is the work the ceiling exists to refuse.
  That is the engine's one deliberate departure from §6.2 ¶2, taken under ECMA-434
  §7.2.5 and said so in both crates' documentation.
- **A `GET` on `/nlip` answers `405` with an NLIP error message**, keeping axum's
  `Allow: POST`, instead of an empty body. ECMA-431 §6 obliges `POST` and forbids
  nothing, so the status is this crate's choice like the rest; the body is the crate's
  principle that a peer reading only the payload learns what happened. A mismatch on any
  other path is still the deployment's fallback.
- **`ServerConfig` carries what ECMA-431 leaves open and a peer can observe:**
  `max_request_body_size`, `correlator_header` (`Option<HeaderName>`, default
  `x-nlip-correlator`, reused from the peer when sane and minted otherwise),
  `ConformityConfig`, `Rejection` and a `StatusMap` whose defaults are the reference
  server's codes — `400` malformed, `400` refused, `413` too large, `501` unsupported,
  `503` unavailable with `Retry-After`, `504` timed out, `500` internal, and `500`
  withheld, the reply the clause-5 sanitiser replaced: the peer asked a valid question
  and got no answer to it, which is the endpoint failing rather than the exchange
  succeeding, and every other way it fails is a `5xx` here. `StatusMap` is
  an exhaustive struct with one field per failure and an `other` for the ones a later
  `EndpointError` variant will bring, rather than a map: a deployment that wants to move
  one code should be told at compile time when a new failure appears, and a lookup that
  silently fell back would not tell it.
- **The body limit is enforced while the body is read**, inside the binding, which
  settles the one rejection the reference server answered out of protocol: a
  `RequestBodyLimitLayer` outside the router refuses in tower-http's own plain text, and
  every refusal this crate can produce is instead an NLIP `MessageType: Error` message
  with the request's tokens echoed into it. jsonrpsee moved the same check inside for
  the same reason.
- **`Server::upload_router(sink)`** mounts ECMA-431 §6.1's out-of-band leg behind an
  `UploadSink` trait, with `DiscardSink` as the one implementation shipped, and its
  documentation states what the reference server left to inference: **clause 6 does not
  apply to it** — the body is arbitrary bytes, so there is no message to validate, no
  tokens to echo and nothing to finish (ECMA-430 §7.3 ¶2).
- **`reject(status, text, retry_after)`** renders the same error body for a deployment's
  own layer that refuses *before* the binding decodes anything, such as a rate limiter.
- **`ServerConfig::principal_header`** — off by default — names the header a trusted
  proxy writes the caller's identity in, and the binding carries its value into
  `Context::extensions` as a **`Principal`** before any layer runs, on the endpoint and
  on the upload leg alike. ECMA-434 §7.2.1's client authentication is then the
  gateway's, as TR/113 §9 allows ("rely on just the base transfer protocol to perform
  the authentication"), and what the value is worth is the network's to establish: the
  binding verifies nothing, a deployment that names no header trusts none, and a header
  that arrives twice, is empty, is not visible ASCII or is longer than
  `MAX_PRINCIPAL_LENGTH` (512 bytes) carries no principal and is logged without its
  value. `Principal` is opaque — what it says is between the proxy and the deployment.
  `nlipsy-component`'s `wasi:http` binding does not read the header yet.
- **`axum` is re-exported**, as `jsonrpsee-server` re-exports `tokio`, because `Router`
  is in the public API. Excluded from `make check-wasm` and from CI's wasm32 job beside
  `nlipsy-http-client`, for the mirror-image reason: axum over hyper over tokio is an
  accept loop and a socket, and a browser has nothing to point one at.

### `nlipsy` — the facade

- **No NLIP code of its own.** It re-exports each `nlipsy-*` crate as a module behind a
  feature flag — `nlipsy::protocol` is `nlipsy-protocol`, as `jsonrpsee::types` is
  `jsonrpsee_types` — so that a consumer gains ECMA-432 or ECMA-433 by adding a word to
  `features` rather than by changing which crate they depend on. Depending on a
  sub-crate directly remains equally supported; the facade is for the consumer's
  manifest, not for their imports.
- **Features, in jsonrpsee's shape:** `protocol`; `client-core`, `http-client`,
  `http-client-hyper` and `client`; `server-core` and `server`; `cbor`; `full`. There
  are **no defaults** —
  `nlipsy = "0.1"` alone compiles an empty crate — because a default would have to
  guess which piece of NLIP a consumer wants and a wrong guess is a dependency they
  cannot drop without turning `default-features` off.
  - `nlipsy::core` appears under *either* `-core` feature and is emitted once, which is
    jsonrpsee's `cfg_client_or_server!`; there is no bare `core` feature, because a core
    with neither half on is an empty crate.
  - `nlipsy::http` is the second such module, under *any* binding feature, and for
    the mirror-image reason: ECMA-431 §6 as values is what a client agent and a server
    agent share, one reading what the other wrote. There is no `http` feature of its
    own — a consumer asks for the role they are playing and the values come with it.
  - `nlipsy::http_client` is the third, and the first whose two features are not two
    roles: `http-client` is ECMA-431 §6 over reqwest and `http-client-hyper` the same
    binding over hyper. Neither implies the other, and the facade takes
    `nlipsy-http-client` with `default-features = false` so that it does not have to —
    which means `nlipsy = { features = ["http-client-hyper"] }` compiles **no reqwest
    at all**, and is therefore a manifest an Android or an iOS build over FFI can
    actually write. (The `default-features = false` sits on the workspace pin rather
    than in this crate's manifest, because cargo refuses to let a member turn a
    workspace pin's defaults off.) `http-client-hyper` is deliberately *not* in `client`
    or in `full`: those mean "every client binding" and "every NLIP capability", and a
    second way to build one binding is neither — a `client` that turned on both would
    put two TLS providers and a C toolchain into the build of everyone who asked for
    the role.
  - `cbor` forwards **weakly** (`nlipsy-core?/cbor` and so on) to every sub-crate that
    is in the build. Without that, `cbor` would reach the message model and stop, and a
    consumer writing `features = ["client", "cbor"]` would get a model that speaks CBOR
    behind a client whose `Wire::Cbor` arm panics.
  - `ws` and `amqp` are deliberately *not* declared — a feature that enables nothing is
    indistinguishable from one that is broken — and are named in the crate's README as
    the shape the table is expected to grow into.
- **`make check-features`**, a new target also run as a CI job, checks each of those
  features on its own, both `-core` features together, both *bindings* together, and
  both client *transports* together. `--all-features` can never catch a feature that
  forgets part of what it enables, because something else has turned that part on;
  thirteen explicit lines, one per manifest a consumer might write, can. The three pairs
  are the three modules that more than one feature produces — a module written as two
  blocks instead of one `cfg(any(..))` fails on those lines and nowhere else. The target
  also carries `nlipsy-http-client`'s own four feature sets and
  `make check-mobile-graph`, which is the dependency-graph gate described in that
  crate's section above.

### The conformance corpus

- **`conformance/` — 125 cases with recorded provenance, in two kinds.** Recorded rather
  than composed wherever the standard prints something to record: a fixture built with
  this workspace's own encoder agrees with a systematic misreading exactly as readily as
  with a correct one.
  - **101 message cases** — bytes in, decode-and-validate out: ECMA-430 Annex A and §5.3
    Table 1, ECMA-432 §7–§8's CBOR frames and text fallback, TR/113's sample exchanges,
    and the readings this workspace takes where the standard is unclear.
  - **24 exchange cases** — clause 6, which no message case can reach, because clause 6
    is about a *pair* of messages and about what an end-point remembers having created.
    Each carries a request, a scripted end-point (its clause-6 configuration, its
    clause-5 policy, and the three answers its token authority gives), the reply that
    end-point's handler produced, and the properties the finished reply must have.
    §6.2's echo including a first-submessage token and its label, §6.2's exception
    meeting §6.2.1's confirmation, minting and not minting, §6.2.2, §6.3 in both
    spellings, §6.4's exchange, the token ceiling on both sides of itself, and the two
    directions of the clause-5 policy. Five of them carry `endpoint.short_circuit`: the
    reply a message-level layer answered with, without the handler and without the step
    that finishes one, where the three idempotent obligations still hold and the
    sanitiser and the minting do not.
  - Running one needs no server, no task and no clock, because clause 6 is two
    synchronous steps over values — which is what keeps the corpus runnable by the
    JavaScript package that comes next.
- **The runner is the unpublished `nlipsy-conformance` member** (`crates/conformance/`)
  rather than a test inside a published crate, so that the crates on crates.io do not
  ship a test whose data is outside the package, and so that a second implementation of
  these clauses can be run against the same cases. It reports per group and per kind,
  and names the kind on every failure line.

### The interface contract and the component

The npm package **`nlipsy`**, with its own version and its own changelog, is
[`javascript/`](javascript/) and is **not published yet**;
[`javascript/CHANGELOG.md`](javascript/CHANGELOG.md) is its own log. `make check-js` is
its gate and it is on: the glue's suites and all 125 conformance cases pass through the
package in Node and in Bun. What does not run yet is three seams of jco —
`wasi:http/handler`, `wasi:http/client`, and a host callback suspended across a turn of
the event loop — whose suites are written, skipped with the reason printed, and run by
`make check-js-e2e`, which is red by design: measured on 2026-10-21 against jco 1.39.0
and preview3-shim 0.11.0, while wasmtime 48 runs every one of these paths today,
`wasi:http@0.3.0` included ([`javascript/README.md`](javascript/README.md), *What runs
today*). The package is not a crate; what it wraps is `crates/nlipsy-component`, which is
`publish = false`. Nothing else in this release depends on it: the crates and the
component are proved by `make check` and `make check-component`, which runs all 125
conformance cases through the artefact in wasmtime.

- **`wit/nlipsy.wit`** — the language-neutral *interface* contract, the way
  `conformance/cases/` is the behaviour one: the message model and the clause-6
  vocabulary as WIT: ECMA-430 clause 5, clause 6's steps and the client half as exported
  functions in three interfaces — `protocol`, `engine` and `client` — and the host's
  end-point, layers, token authority, transport and record sink as five imported
  interfaces of plain functions. No resource crosses the boundary in either direction:
  the context is a record by value, keyed on an `exchange` number. Every item cites its
  clause and the Rust item it is lifted from; `wit/README.md` holds the design, the seed
  table and the proof that every corpus case field is expressible. `make check-wit`
  resolves it and checks every seed.
- **`crates/nlipsy-component`, never published** — the `wit-bindgen` guest of that
  contract, built for `wasm32-wasip3`, where `cargo build` of a `cdylib` emits a
  component with no `cargo-component` and no adapter of any kind, and where `std` links
  WASI **0.3** and nothing else: the artefact's imports are `wasi:cli@0.3.0`,
  `wasi:clocks@0.3.0`, `wasi:http@0.3.0` and `wasi:random@0.3.0`, with no `wasi:io`,
  because 0.3 carries streams natively, and it exports `wasi:http/handler@0.3.0`, so
  the ECMA-431 §6 binding is inside it. The
  target needs a nightly until Rust 1.100 (2026-11-12), which is why the `Makefile` pins
  one in `COMPONENT_TOOLCHAIN`; the crate's own `rust-version` is what the *source*
  needs, not what the target does. It holds no rule: every
  export is `nlipsy-protocol`'s, `nlipsy-core`'s or `nlipsy-http`'s, re-spelled in the
  contract's vocabulary. What it does hold is the composition — the host's layers over the host's
  handler, with `Conformity::finish` at the bottom — and the one thing a guest must
  decide, how the host's `context` reaches a synchronous clause-6 step: each export
  builds one `Context` per call from the record the host passed by value, so two
  exchanges in flight cannot see each other's context by construction. It declares its own `rust-version`, because wit-bindgen 0.62 and edition
  2024 are past 1.85 and nothing published depends on it; every MSRV check excludes it by
  name. The `component` build profile is in the root manifest, because Cargo reads
  `[profile.*]` only there.
- **The corpus through the component.** `nlipsy-conformance` gained a `component`
  feature: the same 125 cases, the same expectations and the same report, answered by the
  built artefact in wasmtime with the contract's five `host-*` interfaces implemented in
  Rust — the first proof of the engine as a component that needs no JavaScript at all.
  The
  runner was refactored rather than forked to make that possible: an `Implementation`
  trait with the walk, the grouping and the report above it, and every expectation now
  compared against what an implementation *produced* (`Observed`, `ObservedRefusal`,
  `ErrorDescription`) instead of against a `Message` the comparison re-derived
  everything from. Beside the corpus it runs two proofs the JavaScript package rests on:
  a synchronous export re-entered while an asynchronous one is suspended on a host
  import, and two exchanges in flight, resolving in reverse order, each keeping its own
  context. Beside those it drives ECMA-431 §6 itself through
  `wasmtime_wasi_http::p3::bindings::Service` — the status line, the body limit, the
  correlator, `405` with `Allow: POST`, `404` in the protocol, `Retry-After`, the
  cross-origin rule — and the client half against a loopback socket.
  `make check-component` is all of it.

- **`examples/echo-endpoint`, never published** — a Rust *end-point* component: the five
  interfaces the engine imports, at their smallest. `wac plug` joins it to the engine and
  what comes out imports only `wasi:*`, exports `wasi:http/handler@0.3.0` and answers
  `curl` under `wasmtime serve` — one artefact, no JavaScript, no host language, no
  framework. `make check-component` builds and composes it and runs a request through the
  result in-process; `make serve-example` puts it on a socket.

### The workspace

- **Nine members, six published.** `nlipsy-protocol`, `nlipsy-core`, `nlipsy-http`,
  `nlipsy-http-client`, `nlipsy-server` and `nlipsy` go to crates.io in that order —
  which is the order they depend on each other — and `nlipsy-conformance`,
  `nlipsy-component` and `echo-endpoint` never leave the repository. A bare
  `cargo build` or `cargo test` at the root covers the six and the corpus runner, the
  manifest's `default-members`; the component and the example are built by name.
- **MSRV 1.85**, verified in CI against the value declared once in
  `[workspace.package]`. `nlipsy-component` and `echo-endpoint` declare their own and are
  excluded by name; both are unpublished, so their floors are promises to nobody.
- **Dual MIT / Apache-2.0 licence.**
- **CI**, in two workflows. `ci.yml` is rustfmt, clippy over all targets and features,
  tests on Linux, macOS and Windows with and without features, the facade's features one
  at a time, the MSRV build, rustdoc the way docs.rs builds it, `wasm32-unknown-unknown`
  over the members that promise it, `cargo-deny`, `cargo-machete`, `cargo-semver-checks`
  and `typos`: what a release waits on, and the only workflow a `v*` tag triggers.
  `component.yml` is the interface contract, the component with the corpus through it,
  and the npm package — on every push to `master`, every pull request, weekly and by
  hand — and gates no release, because what it proves is the experimental part. One
  feature is kept out of every `--all-features` line and named separately instead —
  `nlipsy-conformance/component`, which loads a *built* artefact and would otherwise fail
  for want of a file rather than for want of conformance; each exclusion says so.

[Unreleased]: https://github.com/laktoosivaba/nlipsy/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/laktoosivaba/nlipsy/releases/tag/v0.1.2
