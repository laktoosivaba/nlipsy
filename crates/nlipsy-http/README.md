# nlipsy-http

[![crates.io](https://img.shields.io/crates/v/nlipsy-http.svg)](https://crates.io/crates/nlipsy-http)
[![docs.rs](https://img.shields.io/docsrs/nlipsy-http)](https://docs.rs/nlipsy-http)
[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)
[![licence](https://img.shields.io/crates/l/nlipsy-http.svg)](https://github.com/laktoosivaba/nlipsy#licence)

ECMA-431 §6 as values: the status map, the header spellings, the two directions of
`Retry-After`, and the messages the HTTP binding builds itself.

There is no socket in this crate, no framework, no executor and no clock. It is what the
*client* binding and the *server* binding of ECMA-431 both have to agree on, factored
out of the two so that they cannot disagree — and so that a third implementation of the
same clause, in another language over the same engine, can reach the same rules rather
than re-deriving them from the standard's prose.

ECMA-431 §6 fixes remarkably little: an end-point is at
`https://<server_name>:port/nlip` and accepts a message with `POST`. Everything else an
HTTP binding must decide — which status a refusal carries, what a `Retry-After` means in
both directions, what body a `405` or a `413` has, what the correlation header is called
— is a *choice*, and a choice made twice is a choice made differently. This crate is
where each one is made once, with the clause or the convention it rests on written next
to it.

## What is here

| item | what it is |
|---|---|
| `StatusMap` | one status code per way an exchange can end, with `of(&Outcome, withheld)` and `of_error(&EndpointError)`; the defaults are the reference server's |
| `NLIP_PATH` | the path §6 fixes |
| `CORRELATOR_HEADER`, `correlator_header()` | `x-nlip-correlator`, as a `&str` and as `http`'s `HeaderName` |
| `CONTENT_TYPE` | `application/json` — ECMA-430 §5 ¶2's encoding at ECMA-433 §6.1.5's media type, since §6 names neither |
| `DEFAULT_MAX_REQUEST_BODY_SIZE` | 8 MiB, ECMA-434 §7.2.5 applied to size |
| `retry_after_value` | a `Duration` as the header an end-point writes: seconds, floored at 1 |
| `parse_retry_after` | the header a peer sent, in both RFC 9110 §10.2.3 forms, as a delay from a `now` the *caller* supplies |
| `request` | TR/113 §11.1's chat turn: text in the first submessage, the conversation token beside it |
| `method_not_allowed`, `too_large` | the two error bodies a binding produces before clause 6 is reached |
| `CONVERSATION_SUBFORMAT` | the `conversation` token subformat of ECMA-430 §6.2.1 |
| `cbor` (feature) | forwards to `nlipsy-core/cbor`, for a consumer that also speaks ECMA-432 §7.1 |

```rust
use std::time::{Duration, SystemTime};

use nlipsy_core::{EndpointError, Outcome};
use nlipsy_http::{StatusMap, parse_retry_after, request, retry_after_value, too_large};

// One exchange's status, from the outcome the engine reported. `withheld` is the one
// case where a successful outcome is not a `200`: the end-point answered, and clause 5
// refused to let its answer out.
let codes = StatusMap::default();
assert_eq!(codes.of(&Outcome::Ok, false), 200);
assert_eq!(codes.of(&Outcome::Ok, true), 500);

// A dependency that named a delay is telling the peer when to come back, so the header
// passes it on — floored at one second, because "retry in zero seconds" is an
// invitation to a tight loop.
let unavailable = EndpointError::Unavailable {
    detail: "the model provider answered HTTP 429".to_string(),
    retry_after: Some(Duration::from_secs(12)),
};
assert_eq!(codes.of(&Outcome::Failed(unavailable), false), 503);
assert_eq!(retry_after_value(Duration::from_secs(12)), "12");
assert_eq!(retry_after_value(Duration::ZERO), "1");

// The other direction. RFC 9110 §10.2.3 allows a count *or* a date, so a client must
// read both — and `now` is an argument, not a call, which is what keeps this crate
// free of a clock.
let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_445_412_420);
assert_eq!(parse_retry_after("7", now), Some(Duration::from_secs(7)));
assert_eq!(
    parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
    Some(Duration::from_secs(60)),
);
assert_eq!(parse_retry_after("soon", now), None);

// The messages. §6.2.1 lets either peer create the conversation token, and TR/113's
// chat turn brings the client's own from the first request.
let turn = request("What is Ecma?", "english", "conv-1");
assert!(turn.submessages[0].is_conversation_token());

// And the one refusal that cannot echo anything, because the body was never decoded.
assert_eq!(too_large(8 * 1024 * 1024).message_type.as_deref(), Some("Error"));
```

## Why a crate and not a module

The two ends of ECMA-431 are two crates on purpose: `nlipsy-http-client` compiles
reqwest and `nlipsy-server` compiles axum, and a deployment that is only one of the two
should compile only one of them. Neither may depend on the other — that is the property
the workspace exists to keep, so that ECMA-432's binding never has to depend on
ECMA-431's.

That left the clause itself with nowhere to live, and the previous arrangement said so
out loud: `nlipsy-server` declared `NLIP_PATH` with the comment "`nlipsy-http-client`
declares its own copy rather than importing this one, and the duplication is
deliberate: the two are separate implementations of the same clause". That argument was
honest when the two lived in *different repositories*, where agreement between them was
evidence. In one workspace it stopped being evidence and became a second copy to keep in
step — so the copies are gone and the clause is here, with the conformance corpus and
the recorded fixtures as the witness instead (see `nlipsy-http-client`'s README, "What
this crate no longer witnesses").

The other reason is the one that made this crate now rather than later. The engine is
being packaged for JavaScript over the Component Model, and a `fetch` handler there
needs exactly these values: a status for an outcome, the two `Retry-After` directions,
the `405` and `413` bodies. As functions over values with no I/O they cross the wasm
boundary unchanged; as code inside an axum binding they would have had to be written a
second time, in another language, from the standard's prose.

## What this crate decides, and what it refuses to

**Every status code is a choice, and the type says so.** `StatusMap` is an exhaustive
struct with one field per way to fail, built as
`StatusMap { unsupported: …, ..Default::default() }`. It is not `#[non_exhaustive]`,
unlike most values in this workspace, precisely so that spelling works from outside;
`other` is what absorbs a later `Outcome` or `EndpointError` variant without the struct
having to grow. The defaults are the reference server's, code for code.

**`200` is not a field.** Every code here names a way to fail. A deployment that wants
`201` for a success has left §6 behind in a way one number cannot express.

**No clock.** `parse_retry_after` takes `now` as a parameter. `SystemTime::now()` is a
host call `wasm32-unknown-unknown` does not have, and a rule that reads the clock cannot
be tested against a fixed date — so the caller, which already knows what time it is,
supplies the instant.

**An HTTP-date already past is `Some(Duration::ZERO)`, not `None`.** The end-point did
answer the question; the answer is "now". `None` is reserved for a value that says
nothing usable, which is a different thing for a caller to act on.

**Nothing here encodes a message.** `method_not_allowed()` and `too_large(limit)` return
`Message` values. Which wire they go out on, and with which headers, is the binding's —
ECMA-431 carries JSON, ECMA-432 §7.1 carries CBOR, and this crate takes no view.

## Where this crate sits

Six published crates, in the order they depend on each other:

| crate | is |
|---|---|
| [`nlipsy-protocol`](https://crates.io/crates/nlipsy-protocol) | ECMA-430 clause 5 — what a message *is* |
| [`nlipsy-core`](https://crates.io/crates/nlipsy-core) | ECMA-430 clause 6 — what an end-point *owes* a peer, and the sequence a binding drives, with no runtime in it |
| [`nlipsy-http`](https://crates.io/crates/nlipsy-http) | ECMA-431 §6 as values — this crate, shared by both ends of the HTTP binding |
| [`nlipsy-http-client`](https://crates.io/crates/nlipsy-http-client) | ECMA-431 from a client agent's end — one `POST`, over reqwest |
| [`nlipsy-server`](https://crates.io/crates/nlipsy-server) | ECMA-431 from a server agent's end — one `axum::Router`, clause 6 underneath |
| [`nlipsy`](https://crates.io/crates/nlipsy) | the facade: all of them as modules behind feature flags |

Depending on any of them directly is supported and always will be. The facade is for the
consumer's manifest, not for their imports: this crate appears as `nlipsy::http` under
either the `http-client` or the `server` feature.

`wasm32-unknown-unknown` is in scope here, unlike the two bindings: `make check-wasm`
compiles this crate on every run, which is what keeps a clock or a socket from arriving
in it by accident.

## Minimum supported Rust version

1.85, the edition 2024 floor — the same as the rest of the workspace. Raising it is
treated as a breaking change, and it is checked rather than asserted:
`cargo +1.85 check --workspace --all-targets --all-features` is part of what a release
runs.

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
