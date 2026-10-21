# nlipsy-protocol

[![crates.io](https://img.shields.io/crates/v/nlipsy-protocol.svg)](https://crates.io/crates/nlipsy-protocol)
[![docs.rs](https://img.shields.io/docsrs/nlipsy-protocol)](https://docs.rs/nlipsy-protocol)
[![CI](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml/badge.svg)](https://github.com/laktoosivaba/nlipsy/actions/workflows/ci.yml)
[![licence](https://img.shields.io/crates/l/nlipsy-protocol.svg)](https://github.com/laktoosivaba/nlipsy#licence)

The NLIP message model, and the rules ECMA-430 clause 5 states about it.

NLIP — the Natural Language Interaction Protocol — is an Ecma International
standard for agents that exchange messages written in natural language. This
crate is the smallest useful piece of it: the message itself, and nothing else.

## What is here

A message is a *first submessage* — `format`, `subformat` and `content`, plus an
optional `messagetype` — followed by an ordered list of additional submessages.
The crate decodes that from JSON or CBOR, encodes it back, and says what is wrong
with it.

| module | responsibility |
|---|---|
| `message` | the wire model: `Message`, `Format`, `Submessage` |
| `content` | what a message carries, independent of its encoding: `Content`, `Wire` |
| `error` | what went wrong with a message: `ParseError`, `Violation` |

`Content` is the ECMA-404 value space — string, number, object, array, boolean,
null — plus one variant JSON has no room for: `Bytes`. ECMA-432 §7.1 carries NLIP
over WebSocket as CBOR, where binary content is a byte string and not base64, so a
model built on `serde_json::Value` could not hold what that binding sends. Base64 is
how *JSON* spells those bytes (§6.4, ECMA-432 §7.2), and the crate treats it as
exactly that: a spelling, undone on the way in and restored on the way out.

## Features

| feature | default | what it adds |
|---|---|---|
| `cbor` | off | `Message::from_cbor_slice` and `Message::to_cbor_vec` — ECMA-432 §7.1 |

Nothing else changes with the feature: the model, the decoder and the rules are the
same on either wire. A peer that speaks only ECMA-431 (HTTP, JSON) never encodes a
CBOR byte string and does not compile a codec it cannot reach.

## What is deliberately not here

Each omission is a decision rather than an unfinished edge:

- **No binding.** ECMA-431 (HTTP), ECMA-432 (WebSocket) and ECMA-433 (AMQP) each
  put these messages somewhere. None of them is here, so a second binding does not
  have to depend on the first. The `/nlip` path is ECMA-431 §6's and lives with the
  binding that reads it; so do the `/nlip/ws` endpoint and the fallback to
  `/nlip/ws/text` that ECMA-432 §11 asks for when a frame fails to decode — and so
  does a correlation header such as `X-NLIP-Correlator`, which is a convention of the
  reference server rather than anything ECMA-431 defines. CBOR itself is here, because
  it is an encoding of the message rather than a way of moving it.
- **No AMQP type encoding.** By that same test ECMA-433 §6.1.7's other spelling of a
  payload — AMQP type encodings in a Sequence or Value section, content-type
  `message/x-amqp-list` — would belong here too: it is a third encoding of the
  message, not a way of moving it. It is deferred rather than rejected. §6.1.7 says
  the value is "a list of structured objects" and does not say what the list holds —
  the message as one map, or the first submessage and the submessages as members —
  so implementing it would mean picking a shape and calling it the standard's.
- **No place for an undefined field.** Fields clause 5 does not define are dropped, as
  described below. The one place that costs something is ECMA-432 §10, whose optional
  session management lets a session id travel "in `MessageType` or custom field": the
  first half works (the value is carried verbatim), and the second cannot be
  implemented on this model without keeping the raw frame beside it. Naming the
  omission is the honest half of dropping the field.
- **No deployment.** Nothing here is configurable; there is no default to get
  wrong. Everything a deployment can change belongs to the crate that deploys it.
- **No endpoint behaviour.** Clause 6 — echoing tokens, answering control requests
  — is about what an *end point* must do, not about what a message is. A peer that
  only ever sends messages has no use for it.

## Usage

```rust
use nlipsy_protocol::{Content, Format, Message, Submessage};

// Decode what arrived.
let body = br#"{
    "format": "text",
    "subformat": "english",
    "content": "What is Ecma?",
    "submessages": [
        { "format": "token", "subformat": "conversation", "content": "c1" }
    ]
}"#;

let question = Message::from_json_bytes(body).unwrap();
assert_eq!(question.format, Format::Text);
assert_eq!(question.extract_text(None).as_deref(), Some("What is Ecma?"));
assert_eq!(question.tokens().count(), 1);

// Build one to send back.
let answer = Message::text("An international standards body.")
    .with_submessage(Submessage::token("conversation", "c1"));

assert!(answer.to_json_string().contains("conversation"));

// Binary content is bytes in the model, whichever wire it arrived on. JSON spells
// them in base64 (§6.4); the decoder undoes that at the door and the encoder puts
// it back, so nothing downstream has to know which wire it was.
let recording = Message::from_json_bytes(
    br#"{"format": "binary", "subformat": "audio/wav", "content": "UklGRg=="}"#,
).unwrap();
assert_eq!(recording.content, Content::Bytes(b"RIFF".to_vec()));
assert!(recording.to_json_string().contains("UklGRg=="));
```

Two failures are distinguished, because they are handled differently.
`Message::from_json_bytes` returns a `ParseError` — the message cannot be
interpreted at all, so there is nothing to answer — and every variant names the
place it came from (`submessages[1]: required field \`content\` is missing`), so a
caller never has to guess which submessage was malformed. `Message::validate`
returns `Violation`s instead: the message was understood, but broke a rule, and a
deployment may choose to reject it or merely to log it.

`Message` and `Submessage` also implement `serde::Serialize` and
`serde::Deserialize`, so they can be nested inside a larger document or read by any
codec. That path goes through the same decoder — serde is not a second reading of
clause 5 — but serde's error type can only carry a string, so a caller that wants the
typed `ParseError` should use the direct entry points.

## Case, and the order of submessages

§5 — "Capitalization is not relevant to the name or value of the field." Field
names are matched case-insensitively and format values parsed the same way, while
encoding emits the canonical lower-case form — the one TR/113's examples use. A
field present twice under different casing is rejected rather than resolved: NLIP
declares the names case-insensitive, and choosing one of two spellings would be
inventing a rule the standard does not have.

Every such comparison folds ASCII only. The names clause 5 defines, Table 1's format
values and §6.2's two reserved token prefixes are ASCII words, so nothing the standard
fixes is affected; what is affected is a value the standard leaves to the application,
such as a `<lang>` subformat with non-ASCII letters, which is compared byte for byte —
`FRANÇAIS` and `français` are two subformats here. Folding Unicode case would make the
answer depend on a locale-free mapping the standard never mentions, so the limitation
is stated rather than papered over.

§5.1.5 — "the order of the submessages field is significant." Submessages are kept
in a `Vec` and never reordered. The keys of a `structured` payload are not: ECMA-404
calls an object unordered, so `Content::Object` is a sorted map and the same content
re-encodes to the same bytes every time.

Fields the standard does not define are dropped rather than carried through. Clause 5
lists what a message has; an unknown field has no defined meaning to preserve, and
re-emitting it would mean vouching for something this crate cannot check.

## Readings of the standard this crate takes

ECMA-430 is a short document that does not always agree with itself, ECMA-432 adds a
second encoding to it, and Ecma's own reference Python SDK disagrees with the text in
two further places. Where they disagree, the text governs. These are the readings this
crate takes, so that a user meets them as documented behaviour rather than as a
surprise:

- **Two reserved token prefixes, and only two.** §5.3 reserves `authentication` and
  `conversation`; §6.2 says of every other token subformat that it "may also contain
  a subformat which can be any general string … The use of these tokens is up to the
  application". The SDK emits `authorization` in place of `authentication`, so that
  spelling is application-defined, and `Submessage::is_authentication_token` does
  not recognise it as a credential. Such a token is still decoded, encoded and
  carried like any other — it simply is not taken for one.
- **A token in the first submessage is a token.** §6.2 ¶2 obliges an endpoint to
  return "the same submessage with the exact same format, subformat and content" for
  every submessage whose format is `token`, and §5 ¶2 calls the leading object of a
  message "the first submessage" — so a message that is itself a token is a message
  containing one, and the reference SDK's `create_token` builds exactly that.
  `Message::tokens` cannot yield it, because the first submessage is not a
  `Submessage` value; `Message::is_token`, `is_authentication_token` and
  `is_conversation_token` answer for it instead, mirroring the `Submessage`
  predicates. An endpoint honouring §6.2 has to consult both.
- **Token content is normally a string, and saying so is a warning.** Table 1 calls
  it "an opaque token interpreted by end-points" and stops there; Annex A's schema
  lets `Content` be any type; and §6.2's echo is as well defined for a number as for
  a string, since it asks only that the value come back unchanged. A non-string token
  is therefore reported as `ecma430-5.3/token-content` at `Warning` — off-spec by the
  usual meaning of the word, still interpretable — rather than as the `Error` a
  non-string `text` content gets, where natural language that is not a string has no
  reading at all.
- **`binary` subformats are usually reported, not rejected.** Table 1 requires
  `<content>/<encoding>` with `content` one of `audio`, `image`, `sensor` or
  `generic`, and a shape that departs from it is a `Warning` — the severity this
  crate defines as off-spec but interpretable, which `"subformat": "webm"` is: it
  names an encoding a reader can act on. Figure 9 of TR/113 sends exactly that and
  corroborates the reading without being its authority, a Technical Report being
  informative. An empty encoding part is still an `Error`: the payload type is then
  unknown rather than unusual. The same figure's *payload* is a separate matter — it
  is twenty-one base64 characters long, a length no encoder produces, and it is
  refused. A shape can be tolerated; bytes cannot be invented.
- **`video` is a fifth binary content kind.** Table 1's `binary` rule names `audio`,
  `image`, `sensor` and `generic`, and the same row's examples give `video/.mp3` as
  valid while its note pairs encodings with "video". Reading the examples as part of
  the row is the only way the row agrees with itself, so `video` is accepted with no
  finding; every other kind is `ecma430-5.3/binary-content-kind` at `Warning`.
- **`binary` content that is neither bytes nor text is an error.** §5.1.4 and §5.2.4
  make the same demand of every format — content "must be represented as specified by
  the format and subformat field" — and Table 1 makes `binary` bytes, spelled in
  base64 on the JSON wire (§6.4). An object, array, number, boolean or null there has
  no reading in the text, so `ecma430-5.3/binary-content` is an `Error`, as the `text`
  rule is. A payload carried *by reference* is not the exception it looks like: §6.4
  does define how a large payload is referenced — a `control` request answered with a
  `control` message carrying a `structured` / `uri` submessage — and that is a
  different shape from a URL inside the content. A CBOR *text* string under `binary`
  stays a `Warning` (`ecma432-7.1/binary-content-text`): §7.1 lists a text string
  among the content types, and it may well be base64 the peer did not drop.
- **`control` is spelled three ways in the standard.** §5.1.1 reserves the
  `MessageType` *value* `control`, §6.3 speaks of "the control field set to true",
  and TR/113's sample exchanges use `"control": false`. `Message::is_control`
  accepts either spelling, and the encoder preserves whichever arrived. Nothing here
  invents the other spelling: a constructor writes the field its own clause names,
  and an end-point that owes §6.3's boolean adds it where §6.3 is implemented.
- **§6.4's answer puts the URI in a submessage, not in the first one.** ¶3 asks for
  "a message with MessageType field set to 'control' containing a submessage with the
  format field set of 'structured' and subformat of a URI", and §5.1.4 makes the first
  submessage the message's own content — so the two are different places and the
  clause names the second. `Message::upload_response` builds ¶3 literally: `control`
  as the `MessageType`, a `text` first submessage saying what the address is for, and
  `Submessage::uri` beside it. `Message::uri` stays the general `structured` / `uri`
  first-submessage constructor, which is a different message and not this answer.
- **`format: "error"` is not a format.** Table 1 lists six values and `error` is not
  among them, so the SDK's use of it is not followed. An error indication is
  `MessageType: Error` on a text message — what `Message::error` builds, what
  ECMA-432 §11's own error example sends, and what TR/113 §10 recommends.
- **Base64 is a spelling of bytes, not a kind of content.** §6.4 calls it "JSON
  Base64 encoding"; ECMA-432 §7.2 asks for it only in the text fallback, while §7.1
  puts raw binary in a CBOR byte string and §8's first two examples send `b'…'`
  rather than a string. The model therefore holds bytes, and base64 is undone at the
  door — for `content` under `format: "binary"`, arriving over `Wire::Json`, and
  nowhere else. A `structured` payload that happens to look like base64 stays a
  string, and a text string in a CBOR frame stays text. Decoding there rather than
  lazily is what makes a JSON message re-encode as a CBOR byte string at all.
- **Unpadded base64 is accepted; a different alphabet is not.** Neither §6.4 nor
  ECMA-432 §7.2 says *padded*, so rejecting an unpadded but otherwise well-formed
  payload would enforce a rule that is not in the text and drop a message whose bytes
  are entirely recoverable. Encoding always emits the padded standard form, which is
  what §8 Example 3 shows. The alphabet is not relaxed in the same way: `-` and `_`
  are a different encoding, not a lenient spelling of this one.
- **`content: null` is a value, not an absent field.** Annex A's schema lists `null`
  among the types `Content` may take, so a present-but-null content decodes to
  `Content::Null` and `validate` reports it where the format needs a string. (An
  earlier version of this crate treated it as a missing field, which conflated "the
  peer sent nothing" with "the peer sent nothing *useful*".) The string-typed fields
  — `messagetype`, `label`, `format`, `subformat` — keep the opposite convention:
  §5.1.1 and §5.2.1 say they "must be a string", so a null carries nothing the
  standard could interpret and counts as absent.
- **ECMA-432 §8's own subformats are warnings.** That clause calls its examples
  normative, and two of them do not match Table 1: Example 1 sends
  `"Subformat": "application/json"` under `structured`, where the table's value is
  `json`, and Example 3 sends `"audio/wav;base64"` under `binary`, where the table
  asks for a bare content encoding such as `wav`. Both are accepted and reported —
  making either an `Error` would reject a document the committee published as an
  example of the protocol.
- **An empty `submessages` array is accepted.** §5.1.5 says the field, when present,
  is an array "consisting of one or more submessages", and TR/113 Figures 9–11 — the
  committee's own sample exchanges — all send `"submessages": []`. It decodes to no
  submessages with no finding, and re-encodes as an absent field, since §5.1.5 makes
  the field optional and an empty array says nothing its absence does not.
- **`Label` is accepted on the first submessage.** §5.2.1 gives `Label` to
  submessages and §5.1 does not list it for the first one, while Annex A's schema has
  no top-level `Label` either — but the reference SDK puts one on every message, and
  the field means the same thing in both places. It is decoded, carried and re-emitted
  rather than dropped as an undefined field.
- **An exact repeat of one key resolves last-wins.** Two *casings* of one name are
  refused, as above; two occurrences of the identical name are not, and the difference
  is where the decision is made. Every mainstream JSON parser — `serde_json` here,
  `JSON.parse` in the JavaScript package that is meant to follow — collapses an exact
  repeat while building the object, so the protocol layer is handed one pair and never
  learns there were two. Refusing it would oblige every implementation to write its
  own JSON parser, for a case ECMA-404 assigns no meaning to at all. A repeat under
  two casings survives parsing as two distinct keys, and §5 is what makes those one
  field with two values — a question the protocol has to answer, and does.
- **A CBOR tag in `content` is refused.** RFC 8949 §3.4 tags carry meaning — tag 0
  is a date-time string, tag 32 a URI — that NLIP has no field for and JSON has no
  spelling for. Stripping the tag would hand the application a value whose meaning
  had quietly changed and which could not be re-encoded as what arrived, so the
  message is refused instead. The exception is a bignum (tags 2 and 3), which the
  codec resolves to an integer before this crate sees it, and which is kept when
  ECMA-404 can represent it. A CBOR map key that is not text is refused for the same
  reason: §5 makes a message an ECMA-404 object, whose names are strings. So is a
  number CBOR can hold and ECMA-404 cannot — an integer outside the `i64`/`u64` range,
  or a NaN or ±∞ (RFC 8949 §3.3) — because the alternative is to hand the application
  a value the peer did not send: a rounded figure, or a sensor reading of infinity
  arriving as "no reading". A message whose content cannot be represented has not been
  decoded.
- **A CBOR frame carries exactly one message.** ECMA-432 §7.1: a binary message
  "shall contain a single NLIP message encoded in CBOR". A CBOR reader stops at the
  end of the first item, so anything after it is `ParseError::TrailingBytes` rather
  than silently dropped — which is also what the JSON wire does already, since
  `serde_json` refuses trailing characters.

## How those readings were checked

The oracle is deliberately outside this crate, and ordered: the standard's own
worked examples in Annex A, TR/113 and ECMA-432 §8, bytes recorded from a live
endpoint, and — last, because it is the one source that is not the standard — the
reference SDK.

A test that builds its fixture out of this crate's own encoder agrees with a
systematic mistake in it just as readily as with a correct reading. The checks that
matter therefore live outside the Rust code, in a language-neutral conformance corpus
at `conformance/` in the repository root, which the unpublished workspace member
`nlipsy-conformance` runs against this implementation. The runner is outside this
package too, for the same reason: the corpus cannot travel inside a `.crate`, and a
runner without its cases is a test that fails for whoever builds from the tarball. Each
case records where it came from — the clause or figure of Annex A, TR/113 or ECMA-432
§8 — so that a case can be traced back to the sentence it tests, and so that the same
corpus can be run against a second implementation of the same clause in another
language. What is written here in prose is what that corpus is expected to assert.

## Status

Version 0.1.2, the first on crates.io. The API is not frozen — `Content` and `Wire`
replaced a `serde_json::Value` before the first release rather than after it, which is
the kind of change this status exists to permit. `Message`, `Submessage`,
`ParseError` and `Violation` are `#[non_exhaustive]`: construct them through their
constructors and match them with `..`, because fields will be added.

The crate was extracted from a working NLIP server deployment, where it is the wire
model shared by the server and its terminal client, and that deployment remains its
first consumer — what it is exercised against, and a second reading of §5 rather than
an independent one. Two implementations of the same clause find each other's mistakes;
they do not find the mistakes they share.

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
["protocol"] }` is this crate, under the name `nlipsy::protocol`.

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

ECMA-430, ECMA-431, ECMA-432 and TR/113 are publications of Ecma International and
are not redistributed here.
