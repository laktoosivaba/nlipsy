# NLIP conformance corpus

Recorded NLIP messages with the outcome every implementation must produce for them,
and clause-6 exchanges with the reply every implementation must produce for them.

`nlipsy-protocol` claims to be the single point of truth about ECMA-430 for Rust, and
a JavaScript package is meant to follow. A claim like that is worth what it can be
checked against, so the rules do not live only in Rust: they live here, as data, and
the Rust crate is simply the first implementation that runs them. A second
implementation that passes this corpus agrees with the first about every question the
corpus asks — not because it read the same prose, but because it produced the same
answers for the same bytes.

Nothing in here is code. A case is a recorded input and the expected outcome; the
runner belongs to the implementation being tested. The Rust runner is the workspace
member `crates/conformance` (`nlipsy-conformance`), which is never published — the
corpus lies outside every package, so a runner that shipped with a crate would ship
without its cases. It walks this directory, runs every case, and prints each failure on
its own line before failing once with the count:

```
cargo test -p nlipsy-conformance --all-features
```

CBOR cases need the member's `cbor` feature, which forwards to `nlipsy-protocol/cbor`;
without it they are skipped and counted.

## Two kinds of case

**125 cases: 101 message cases and 24 exchange cases.**

A **message case** is bytes in, decode-and-validate out. It can say everything about
what a message *is* — ECMA-430 clause 5 — and nothing about what an end-point must
*do* with one, because clause 6 is about a pair of messages and about what this
end-point remembers having created.

An **exchange case** is the second kind, added for clause 6. It carries a request, a
scripted end-point (its clause-6 configuration, its policy, and the three answers its
token authority gives), the reply that end-point's handler produced, and the properties
the *finished* reply must have. Nothing in it needs a network, a task or a clock:
clause 6 is two synchronous steps over values — one before the handler, one after it
— and an exchange case is those two steps applied to data.

Which kind a file is, is its `kind` field: `"exchange"`, or `"message"`, or absent,
which means `message`. Every case written before the second kind existed is in that last
state, and they were left alone rather than migrated: those files are evidence about the
protocol, and editing every one of them to satisfy a schema would make them evidence
about a schema.

## Layout

```
conformance/
  README.md                 this file
  cases/
    ecma-430/               the protocol itself: Annex A, §5.3 Table 1, §6 —
                            33 message cases and 10 exchange cases
    ecma-432/               the WebSocket binding: CBOR wire, text fallback, errors
    tr-113/                 the explanatory report's sample exchanges
    readings/               the readings the crate takes where the standard is unclear,
                            the negative cases that pin them, and 14 exchange cases for
                            the clause-6 questions §6 leaves open
  tools/
    cbor_encode.py          produces the `input_hex` of a CBOR case from a JSON
                            description, so those bytes are reproducible
```

A case's number is a stable identifier and is never reused, so the numbering inside a
group has gaps: `ecma-430/` has no 33 or 39 today, and `readings/` has no 42. **A gap
means a case moved between groups, never that one was deleted** — what a case records
is evidence about the standard, and the group it sits in is a claim about whether the
expectation is the clause's or this implementation's reading of it. When that claim turns out to be wrong
the file moves and keeps its content, its provenance and its history; the group it
arrived in gives it a new number there and the old number stays spent. The three moves
so far are `ecma-430/33` → `readings/49`, `ecma-430/39` → `readings/50` (both were
readings pinned as rules) and `readings/42` → `ecma-430/41` (a reading the engine's
review overturned into a rule).

The two kinds share one sequence inside a group, so the next free number is the one past
the highest of **both**: `ecma-430/42` is a message case and `ecma-430/41` an exchange
case, and a new case of either kind in that group starts at 46.

§6.4's out-of-band exchange has a shape worth naming, because it is easy to build the
wrong one: the response is a message with `MessageType` `control` **containing a
submessage** whose format is `structured` and whose subformat is `uri` — a control
message carrying the URI in a submessage, not a bare `structured`/`uri` message.
`cases/ecma-430/27-out-of-band-response.json` records it, and in `nlipsy-protocol` it
is `Message::upload_response(..)` — which builds exactly that, so the shape is named
once in the crate rather than assembled at every call site.

There is no `ecma-433/` group. ECMA-433 §6 maps NLIP onto AMQP sections, properties
and content types; it contains no NLIP message body to record. What it says about the
payload — that JSON payloads travel in a Data section with content-type
`application/json` — is a binding concern and is tested where the binding lives.

## A message case

One JSON file per case, at `cases/<group>/<nn>-<slug>.json`. This is
`cases/ecma-430/09-missing-subformat.json` in full:

```json
{
  "title": "Required field `subformat` missing",
  "source": {
    "document": "ECMA-430",
    "clause": "§5.1.3, Annex A required [\"Format\", \"Subformat\", \"Content\"]",
    "note": "composed: negative case for §5.1.3, subformat missing from the first submessage."
  },
  "wire": "json",
  "input": "{\n  \"Format\": \"text\",\n  \"Content\": \"What is Ecma?\"\n}",
  "expect": {
    "decode": "error",
    "error": {
      "kind": "missing-field",
      "path": "$",
      "field": "subformat"
    }
  },
  "notes": "optional; a doubt about the expectation, a conflict with the standard, or a defensible alternative reading"
}
```

| field | meaning |
|---|---|
| `title` | one line, what the case is about |
| `source.document` | `ECMA-430`, `ECMA-432`, `Ecma TR/113`, `ECMA-404` |
| `source.clause` | clause, figure or example number the input comes from |
| `source.note` | provenance — see [Provenance](#provenance) |
| `wire` | `json` or `cbor` |
| `input` | JSON wire: the document as raw text, in one string |
| `input_hex` | CBOR wire: the frame's bytes in lower-case hex |
| `expect` | see [Expectations](#expectations) |
| `notes` | optional; doubt, conflict, or a defensible alternative reading |

`input` is text and not a nested JSON value on purpose. Casing, duplicate keys,
whitespace and base64 padding are all things this corpus has cases about, and a parsed
value would have thrown every one of them away before the implementation under test
ever saw them. CBOR inputs are hex for the same reason: map order and definite-length
heads are part of what was recorded.

## Expectations

`expect.decode` is `"ok"` or `"error"`.

### On error

```json
"expect": { "decode": "error", "error": { "kind": "...", "path": "...", "field": "..." } }
```

Exactly one error is expected: decoding stops at the first thing it cannot interpret.
`field` is present only for the kinds that name one, and is always the canonical
lower-case spelling (`subformat`, never `Subformat`). A `base64` error always names
`content`, which is the only field it can be about. `path` is `$` for the first
submessage or the document as a whole, and `submessages[i]` for the i-th submessage,
counted from zero.

An implementation is not expected to reproduce the *message* of an error, only its
kind, path and field.

| `kind` | meaning |
|---|---|
| `invalid-json` | the body is not JSON at all (`path` is always `$`) |
| `cbor` | the frame is not CBOR, carries a value NLIP has no room for — a tag, a map key that is not text, a non-finite float — or holds more than the single NLIP message §7.1 allows (`path` is always `$`) |
| `not-object` | valid JSON/CBOR, but the message or a submessage is not an object |
| `missing-field` | a field §5.1.2–5.1.4 / §5.2.2–5.2.4 requires is absent, or present as `null` where a string is required |
| `wrong-type` | a field is present with a type the standard does not allow there |
| `duplicate-field` | one field name arrived under two casings, which §5 makes one field with two values |
| `unknown-format` | `format` is not one of Table 1's six values |
| `base64` | `binary` content on a JSON wire is not decodable base64 |

### On success

```json
"expect": {
  "decode": "ok",
  "message": { ... },
  "violations": [ { "rule": "...", "severity": "...", "path": "..." } ],
  "is_control": false,
  "text": "What is Ecma?",
  "tokens": [ { "subformat": "...", "content": "...", "kind": "..." } ],
  "token_count": 2,
  "first_submessage_token": { "subformat": "...", "content": "...", "label": null },
  "canonical_json": "{\"format\":\"text\",..."
}
```

- **`message`** — the decoded model, spelled as described below.
- **`violations`** — every finding `validate` reports, as `{rule, severity, path}`.
  Compared as a set: order is not part of the expectation, and duplicates do not occur.
  An empty list means the message is clean, and is not the same as an absent key.
- **`is_control`** — `true` when `messagetype` equals `control` ignoring case, **or**
  the `control` field is boolean `true`. §5.1.1 makes control a reserved *value* of
  `messagetype`; §6.3 makes it a boolean *field*, which is the spelling TR/113's figures
  send (`"control": false`). Both are read, and the two are OR-ed. See `readings/22`,
  where they disagree inside one message.
- **`text`** — all text content of the message joined by single spaces, in document
  order: the first submessage first if its format is `text`, then every `text`
  submessage in the order it arrived (§5.1.5). Any language. `null`, not `""`, when
  there is no text at all.
- **`tokens`** — optional, present when the message carries token submessages. One
  entry per `token` submessage in the `submessages` array, in order; the first
  submessage is not included, because a token there would be the message itself rather
  than an annotation on it. `kind` is `conversation` or `authentication` when the
  subformat begins with that prefix ignoring case, and `other` for every other
  subformat — §5.3 reserves exactly two prefixes and §6.2 leaves the rest to the
  application. The exclusion is about this list, not about §6.2: a message whose
  *first* submessage is a token still contains a token submessage the clause obliges an
  end-point to return, so an implementation needs a predicate over the first submessage
  beside the list. `readings/36` is that case, and its `tokens` is an empty array.
  The list is also what the *receiving* end-point owes: §6.2 ¶2 obliges it to include
  every one of these submessages in its response "with the exact same format, subformat
  and content", which is the subformat as recorded here and not a re-spelling of it.
  `ecma-430/42` is that case read from the client's side — a reply whose two tokens are
  what the client returns on its next request.
- **`token_count`** — optional; how many `token` submessages the message carries, the
  first submessage **included**. Exactly the number `tokens` cannot give, which is why it
  is a key of its own rather than something a runner derives: the two differ by one on
  precisely the message §6.2 is easiest to get wrong about, and it is the count the token
  ceiling is applied to, so an implementation that derived it from the list would refuse
  the wrong requests. `readings/36` is `token_count: 1` with an empty `tokens`;
  `ecma-430/23` is four and `ecma-430/42` is two, where the two agree.
- **`first_submessage_token`** — optional; the first submessage as a `token`
  submessage, spelled `{subformat, content, label?}`, or an explicit `null` for "the
  first submessage is not a token". Absent asserts nothing, as everywhere else; `label`
  is optional inside it, and an explicit `null` there asserts there is none. `format` is
  not a field, because `token` is the only value it could hold. This is the predicate
  §5 ¶2 forces on an implementation — a message that *is* a token contains a token
  submessage for §6.2's purposes — asserted directly rather than through the echo it
  causes; `readings/36` records it, and `readings/41` is the same message put to an
  end-point.
- **`canonical_json`** — optional; the exact string a re-encoder must produce.

### The canonical model

`message` is the decoded message as JSON, so that implementations in different
languages can compare against the same text:

- field names `message_type`, `control`, `label`, `format`, `subformat`, `content`,
  `submessages`; a submessage has `label`, `format`, `subformat`, `content`.
- absent optional fields are **omitted**, never spelled as `null`. `content: null` is
  a present content whose value is null (Annex A allows it), which is a different thing
  from an absent one.
- `format` is the lower-case Table 1 value, whatever casing arrived.
- `subformat`, `label` and `message_type` keep the casing they arrived in: §5 makes the
  *format* value case-insensitive, and says nothing that would let an implementation
  rewrite an opaque token or a language tag.
- `submessages` is omitted when empty.
- byte-string content is `{"$bytes": "<standard base64, padded>"}`, which is what
  distinguishes it from a JSON string. Structured content is reproduced as it arrived.
  (A structured content that is itself an object with a single `$bytes` key would be
  ambiguous under this spelling; no case has one, and the corpus would need a different
  convention if one ever arrived.)
- `message` is compared as a JSON *value*, not as text: the order of an object's keys —
  the model's own fields, and the keys inside structured content — carries nothing.
  `canonical_json` is where key order is asserted.

### `canonical_json`

The re-encoding of the model onto the JSON wire, as one exact string:

- lower-case field names, in a fixed order: `messagetype`, `control`, `label`,
  `format`, `subformat`, `content`, `submessages`; within a submessage `label`,
  `format`, `subformat`, `content`. Absent fields are skipped, and `submessages` is
  omitted when empty.
- object keys **inside** structured content are sorted; the content's own arrays keep
  their order.
- bytes are emitted as standard-alphabet base64 **with** padding, even when the input
  was unpadded.
- compact: no space after `:` or `,`, no newlines.
- UTF-8, and only the escapes ECMA-404 requires: `\"`, `\\`, `\b`, `\f`, `\n`, `\r`,
  `\t`, and `\u00XX` for the remaining control characters. Non-ASCII characters are
  emitted literally.
- numbers are re-emitted in the shortest form that round-trips. No case uses a number
  whose spelling this could change; if one is added, it needs a note.

Decoding a `canonical_json` and re-encoding it yields the same string for every case in
the corpus, and the re-decoded model equals `message` — except for
`ecma-432/06` and `ecma-432/07`, where the CBOR wire distinguishes a byte string from a
text string and the JSON wire cannot. Both cases say so in their `notes`.

## An exchange case

One JSON file per case, in the same tree and the same numbering as the message cases.
This is `cases/ecma-430/32-conversation-token-minted.json`, trimmed of its `notes`:

```json
{
  "kind": "exchange",
  "title": "No conversation token from the peer, so the end-point creates one",
  "provenance": {
    "standard": "ECMA-430",
    "clause": "6.2.1",
    "quote": "Either the client agent or the server agent may be the first one to create a submessage with format field of 'token' and the subformat field starting with 'conversation'. ..."
  },
  "endpoint": {
    "config": { "issue_conversation_token": true },
    "authority": { "mint": { "conversation": "srv-new" } },
    "reply": { "format": "text", "subformat": "english", "content": "An international standards body." }
  },
  "request": { "format": "text", "subformat": "english", "content": "What is Ecma?" },
  "expect": {
    "outcome": "reply",
    "reply": { "tokens": [ { "format": "token", "subformat": "conversation", "content": "srv-new" } ] },
    "findings": []
  }
}
```

| field | meaning |
|---|---|
| `kind` | always `exchange` |
| `title` | one line, what the case is about |
| `provenance.standard` | `ECMA-430` or `ECMA-431` |
| `provenance.clause` | the clause, as `6.2`, `6.2.1`, `6.3`, `6.4` |
| `provenance.quote` | the sentence the expectations are read out of |
| `endpoint` | the end-point the request is put to — see below |
| `request` | the peer's message, as a JSON object |
| `expect` | see [Exchange expectations](#exchange-expectations) |
| `notes` | optional; doubt, conflict, or a defensible alternative reading |

`provenance` rather than the message cases' `source`, and the difference is not
cosmetic. `source` answers "where did these bytes come from", and its
`verbatim` / `composed` distinction is what separates evidence about the protocol from
evidence about an implementation's opinion. An exchange case has no recorded bytes to
be honest about: ECMA-430 prints no worked clause-6 exchange anywhere, so every request
here is composed and saying so would carry no information. What is worth recording
instead is the *rule* — which clause, and which sentence of it the expectations claim
to follow — so that a reader who disagrees with a case can disagree with the reading
rather than with the corpus.

`request` and `endpoint.reply` are JSON **objects**, where a message case's `input` is
raw text. The reason `input` is text — casing, duplicate keys, whitespace and base64
padding are all things the corpus has cases about — is a reason about decoding, and
decoding is not what an exchange case tests. Every one of those questions already has a
message case; an exchange case starts from a message that decoded, and an object is
what a reader can check against the clause.

### The end-point

```json
"endpoint": {
  "config": {
    "issue_conversation_token": false,
    "issue_authentication_token": false,
    "max_tokens_per_message": 32
  },
  "rejection": {
    "inbound":  { "on_error": "reject", "on_warning": "log" },
    "outbound": { "on_error": "reject", "on_warning": "log" }
  },
  "authority": {
    "own": ["srv-old"],
    "issued": { "conversation": "srv-1" },
    "mint":   { "conversation": "srv-new" }
  },
  "reply": { ... },
  "short_circuit": false
}
```

Every block is optional and every value above is its default, so a case states only
what it is about.

- **`config`** — §6.2.1's and §6.2.2's issuance switches, both off, because §6.2's echo
  is the only mandatory half of the clause: an end-point that keeps no session state
  creates nothing and is equally conforming. `max_tokens_per_message` is a ceiling on
  the token submessages one request may carry; it is **not** in the standard, and what
  happens at it is (see `readings/50` and `ecma-430/40`).
- **`rejection`** — what a clause-5 finding *does*, per direction and per severity, over
  `reject`, `log` and `ignore`. Also not in the standard: ECMA-430 states the rules and
  says nothing about the consequence. Two directions rather than one, because an
  end-point that tolerates a peer's warnings has no reason to tolerate its own.
- **`authority`** — the three answers §6.2's exception and §6.2.1's confirmation need
  from the end-point's own memory.
  - `own` is the set for which "was this token created by this end-point?" answers yes.
  - `issued` is what the end-point is currently confirming, per kind — the token it owes
    back on *every* reply, including the reply to the message that carried it.
  - `mint` is what it would create, per kind, if it created one. A kind absent here
    mints nothing, which is how a case says "creation was possible and did not happen".
  - The three are deliberately independent, because the configurations that catch bugs
    are the ones where they disagree: a token that is `own` but no longer `issued` is
    what a peer holds after the conversation behind it was evicted. A value returned by
    `mint` joins `own` and `issued` from that moment, because from that moment the
    end-point did create it.
- **`reply`** — the message the end-point's *handler* produced, before clause 6 touched
  it. Absent on a case whose outcome is `refused`: the refusal happens before the
  handler is asked anything, so there is no reply for the case to invent.
- **`short_circuit`** — `reply` is what a *layer* answered, without the handler and
  without the step that finishes one. See
  [A short-circuited reply](#a-short-circuited-reply).

### Exchange expectations

```json
"expect": {
  "outcome": "reply",
  "refused": { "kind": "too-many-tokens", "count": 33, "limit": 32 },
  "reply": {
    "messagetype": "control",
    "control": true,
    "first_submessage": { "format": "text", "subformat": "english" },
    "tokens": [ { "format": "token", "subformat": "conversation", "content": "c-1", "label": null } ],
    "contains": [ { "format": "structured", "subformat": "uri" } ],
    "canonical": { ... }
  },
  "withheld": false,
  "findings": [ { "direction": "inbound", "rule": "...", "severity": "warning", "path": "..." } ]
}
```

`expect.outcome` is `"reply"` — the end-point answers — or `"refused"`, when the
request is turned away before the handler runs.

- **`refused`** — present when `outcome` is `refused`. `kind` is `violations` (the
  request breaks clause 5 under a policy that rejects) or `too-many-tokens`. `rules` is
  optional and lists the rule ids behind a `violations`, as a set; `count` and `limit`
  are optional and belong to `too-many-tokens`.
- **`reply`** — present when `outcome` is `reply`. Every field in it is optional and
  each asserts one thing, so that a case says what its clause is about and stays silent
  about the rest: a second implementation must not be failed for a choice the standard
  leaves it.
  - `messagetype` and `control` are compared exactly. An explicit `null` asserts the
    field is **absent** from the reply, which is a different claim from not mentioning
    it at all.
  - `first_submessage` is the reply's own format and subformat — §5.1.4's content, the
    end-point's answer. It is worth asserting because §6.2's echo must not displace it:
    a peer whose whole message was a token gets an answer with its token beside it,
    not an answer replaced by it.
  - `tokens` lists the `token` submessages of the reply's `submessages` array, by
    format, subformat and content — the three §6.2 calls "the exact same". `label` is
    optional: absent asserts nothing about one, and an explicit `null` asserts there is
    none.
  - `contains` names submessages that must be present by format and subformat, without
    saying what else the reply may carry. §6.4 ¶3's `structured`/`uri` is what it is
    for.
  - `canonical` is the whole finished message in the canonical model spelling above,
    order included. Used only where the order *is* the reading being recorded.
- **`withheld`** — optional; `true` when clause 5 rejected the end-point's *own* answer
  and `reply` is the replacement rather than the answer. Absent asserts nothing, which
  is what almost every case says about it: the flag is not a property of the message,
  and a case about a clause of ECMA-430 has no business pinning it. It is in the schema
  because a binding reads it and nothing else in the corpus does — it is what an
  ECMA-431 binding turns into a status line — and because it is **not** derivable from
  `reply`: a replacement error message and a handler that answered with an error message
  of its own are the same bytes on the wire. `readings/45` and `/46` are `true`,
  `ecma-430/30` is `false`, and both values are pinned so that an implementation
  answering one of them always cannot pass. A `short_circuit` case may not say `true`;
  see below.
- **`findings`** — every clause-5 finding the exchange reported, as
  `{direction, rule, severity, path}`, compared as a multiset. `direction` is `inbound`
  (about the peer's request, reported by the step before the handler) or `outbound`
  (about the end-point's own reply, reported by the step after it) — the same split
  `endpoint.rejection` has, and for the same reason. `rule`, `severity` and `path` are
  the vocabulary of [Rule ids](#rule-ids), shared with the message cases. An absent key
  asserts nothing; an empty list asserts a clean exchange.

### Why `tokens` is a multiset and not a sequence

§6.2 ¶2 says which submessages must come back — "the same submessage with the exact
same format, subformat and content" — and says nothing about **where** in the reply
they go. Asserting the order would fail a conforming implementation for a choice the
clause leaves it, so `tokens` is matched by membership: each expected token must be
somewhere in the reply's token submessages, and nothing else may be.

This implementation does choose an order — the request's *first* submessage token
leads, then the tokens of the `submessages` array in arrival order, on the reading that
§5.1.5 makes the order of submessages significant and that reproducing the peer's order
is most likely to mean what the peer meant. That choice is recorded in exactly one
case, `readings/48`, through `expect.reply.canonical`. A second implementation that
fails only that case and passes every other has made a different choice and not a
mistake; the right response is to say so in that file, not to change code.

### A short-circuited reply

A message-level layer — a cache, a canned answer, a maintenance-mode layer — can answer
a request without calling the rest of the stack, so the step that finishes a handler's
answer never runs. The standard knows no layers: ECMA-430 names an end-point and says
what its response message must contain, and says nothing about which part of an
implementation produced it. So what such a reply owes the peer is a **reading**, and
this corpus takes the one that makes the clause about the message the peer receives:
§6.2's echo, §6.2.1's confirmation of the token the end-point has already issued, and
§6.3's carry hold on the wire whoever produced the reply. Every case that records it is
therefore a `readings/` case whose `notes` state the reading.

`"short_circuit": true` in `endpoint` is the shape. It says `endpoint.reply` is not the
handler's answer but a layer's, and it changes step 4 of the algorithm below: only those
three obligations run, and the two that are not idempotent do not.

- **No sanitiser.** The three obligations are re-applied to *every* reply, whether or
  not the finishing step ran; clause 5 is not, and the reason is what would happen on the
  ordinary path. There the reply has already been sanitised and already carries the
  peer's echoed tokens, so a second pass would report the peer's own `token-content`
  warning again — and under an outbound policy of `on_warning: reject` it would replace
  a good reply with an error. The price is paid here: a reply the finishing step never
  saw is not sanitised at all. The consequence — a layer that writes a malformed reply
  sends one — is recorded rather than hidden (`readings/55`), and an implementation that
  does sanitise here has made a different choice and not a mistake.
- **No issuance.** Minting is a write, and a deployment that persists what it minted
  commits on the path where its handler ran. Creating an identifier on a path that never
  commits would hand the peer a conversation the end-point has no record of, so creation
  stays with the handler's step (`readings/54`).
- **No findings of its own.** There is no outbound step to report one, so
  `expect.findings` on such a case lists the `inbound` findings alone.
- **`expect.withheld` must be absent or `false`.** Nothing ran that could withhold
  anything. A `short_circuit` case that expects `true` is a malformed case and a runner
  must say so rather than run it.

`readings/51` to `/55` are the five: the echo, the confirmation, the control carry, the
minting that does not happen, and the sanitising that does not either.

### Running an exchange case, in any language

For each file whose `kind` is `exchange`:

1. Decode `request` and, if present, `endpoint.reply`, with the implementation's own
   decoder.
2. Build the end-point's clause-6 configuration and policy from `endpoint.config` and
   `endpoint.rejection`, and a token authority whose three answers come from
   `endpoint.authority`.
3. Run the step that happens **before** the handler: apply clause 5 to the request
   under the inbound policy, then the token ceiling. If it refuses, compare the refusal
   with `expect.refused` and stop; its findings are the `inbound` ones.
4. Otherwise run the step that happens **after** the handler over `endpoint.reply`:
   sanitise it against clause 5 under the outbound policy, then §6.2's echo, then
   §6.2.1/§6.2.2's issuance, then §6.3's carry. Its findings are the `outbound` ones.
   When `endpoint.short_circuit` is `true` this step is a different one: `endpoint.reply`
   is what a layer answered without the handler; run only §6.2's echo, §6.2.1's
   confirmation of what is already issued and §6.3's carry over it — no sanitiser, no
   issuance — and it reports no findings.
5. Compare the finished message with `expect.reply`, `expect.withheld` if the case has
   one, and both sets of findings with `expect.findings`.

The order in step 4 is itself load-bearing and is the reading `readings/45` records:
sanitising *first* means a reply replaced by an error message still carries the echo,
and an implementation that sanitised last would drop the peer's tokens on exactly the
path where the peer most needs them back.

Neither step consults a network, a clock, a task or a peer address, which is why an
exchange case is a file rather than a test server. Nothing in the schema names the
peer or the correlator for the same reason: clause 6 does not read them, and a field
that changed no outcome would be a field a reader trusts.

## Rule ids

`violations` uses stable ids, so a finding can be asserted without matching prose.
`severity` is `error` — the message cannot be interpreted as written — or `warning` —
it is off-spec but usable, and a deployment decides what to do about it.

| rule | severity | meaning |
|---|---|---|
| `ecma430-5.2.3/subformat-empty` | error | `subformat` is empty or only whitespace, so the content has no subcategory (any format) |
| `ecma430-5.1.4/text-content` | error | content of a `text` part is not a string |
| `ecma430-5.3/binary-encoding` | error | the part of a binary subformat after `/` is empty or only dots, so the payload has no type (`image/`, `image/...`) |
| `ecma430-5.3/structured-subformat` | warning, informational | a `structured` subformat that is not json, uri, xml or html. Table 1's fifth value is the name of any programming language, which no implementation can enumerate, so a conforming `python` and an off-spec `application/json` are indistinguishable here and the finding reports the fact rather than a fault |
| `ecma430-5.3/binary-subformat` | warning | a binary subformat with no `/`, e.g. TR/113 Figure 9's `webm` |
| `ecma430-5.3/binary-content-kind` | warning | the part before `/` is not audio, image, sensor, generic or video |
| `ecma430-5.3/location-subformat` | warning | a `location` subformat that is neither `text` nor `GPS` |
| `ecma430-5.3/token-content` | warning | content of a `token` part is not a string. Table 1 calls it an opaque token and does not fix its type, and §6.2's echo works for any value, so this is reported rather than refused |
| `ecma430-5.3/binary-content` | error | binary content that is neither bytes nor a string. §5.1.4 makes the same demand of every format and Table 1 makes `binary` bytes; §6.4's by-reference mechanism is a `control` exchange with a `structured`/`uri` submessage (`ecma-430/27`), not a URL inside the content |
| `ecma430-5.3/binary-encoding-parameter` | warning | the encoding part carries a parameter rather than being a bare encoding, e.g. ECMA-432 §8 Example 3's `audio/wav;base64` |
| `ecma432-7.1/binary-content-text` | warning | binary content arrived over CBOR as a text string where §7.1 expects a byte string |

All comparisons of subformat values are case-insensitive. Warnings where Table 1 reads
like a requirement are deliberate: the committee's own examples break the table
(`video/.mp3` in the table itself, `webm` in TR/113 Figure 9, `application/json` in
ECMA-432 Example 1), and an implementation that rejected them could not talk to the
implementations those documents describe. An empty encoding is still an error, because
then the payload's type is unknown rather than unusual.

## Wires

**JSON** (ECMA-430 §5, ECMA-431, ECMA-432 §7.2). `binary` content is a base64 string
and is decoded to bytes at decode time — §6.4 calls this "JSON Base64 encoding". The
standard alphabet only (`+` and `/`, not `-` and `_`); padding is accepted but not
required on input, and always emitted on output. Anything else is a `base64` error.

**CBOR** (ECMA-432 §7.1). Content may be a text string, a byte string, an array or a
map. Binary content is a byte string, carried as-is with no base64 anywhere. A text
string under `binary`, or a byte string under `text`, decodes to what it is and is
reported by `validate`. What CBOR can carry and NLIP cannot is refused as a `cbor`
error rather than coerced: a tagged value (RFC 8949 §3.4) has a meaning neither §5 nor
ECMA-404 has a field for, a map key that is not text is not an ECMA-404 object name,
and a NaN or ±∞ (RFC 8949 §3.3) is not an ECMA-404 number — reading one as null would
deliver a value the peer never sent. The exception is a bignum (tags 2 and 3), which
the codec resolves to an integer and which is kept whenever ECMA-404 can hold it.

A frame carries one message and no more: §7.1 says a binary message "shall contain a
single NLIP message encoded in CBOR", so bytes after the first CBOR item are a `cbor`
error rather than something to ignore (`ecma-432/14`). The JSON wire needs no such
rule — a JSON parser already refuses trailing characters — and the two wires answering
alike is the point.

Field names are matched case-insensitively on both wires (§5), and so are `format`
values. Fields the standard does not define are ignored. A field that arrives twice
under two casings is a `duplicate-field` error: §5 makes them one field, and nothing in
the standard says which value wins.

## Provenance

Every message case's `source.note` begins with one of three words, and the distinction
matters: a fixture copied from the standard is evidence about the protocol, and a
fixture someone wrote is evidence about an implementation's opinion. (An exchange case
has `provenance` instead, and no such word, because every one of its requests is
composed — see [An exchange case](#an-exchange-case).)

- **verbatim** — copied from the document, byte for byte, whitespace included. Ecma's
  copyright licence permits this for "works by making use of this specification in
  standard conformant products by implementing (e.g. by copy and paste wholly or
  partly) the functionality therein". The standards themselves are not redistributed
  here.
- **verbatim, minimally repaired** — copied, then repaired where PDF extraction broke
  the document: a string split across a page or the end of a code fence, a typographic
  quote in place of a straight one, a transposed `}]`. The note says exactly what was
  changed. Nothing is changed that the document got right.
- **composed** — written for this corpus, with the note saying why: a negative case for
  a clause, a payload the standard prints as `...`, or a reading the crate takes where
  the standard is silent.

Where a standard's own example contradicts the semantics the corpus encodes, the case
records the example as printed, expects what the semantics say, and explains the
conflict in `notes`. It is not silently repaired. The three worth knowing about:

- `tr-113/08` — Figure 9's base64 payload has 21 data characters, which no base64
  string can have. The committee's own figure does not decode.
- `ecma-432/01` — Example 3's payload ends in `...`, which is not base64 either.
- `readings/22` — `"control": false` and `"messagetype": "CONTROL"` in one message.
- `ecma-432/02` and `readings/11` — `audio/wav;base64` is ECMA-432 Example 3's binary
  subformat, where `;base64` is a parameter rather than the content encoding Table 1
  asks for, and where §7.2 has already fixed base64 for the text fallback. Reported as
  `ecma430-5.3/binary-encoding-parameter`, not rejected: §8 calls its examples
  normative.

## Running the corpus

The algorithm, for an implementation in any language. In this repository it is
`crates/conformance/src/lib.rs`, and `cargo test -p nlipsy-conformance --all-features`
is how it is run. For each file under `cases/`, read its `kind` first: `exchange` goes
to [Running an exchange case](#running-an-exchange-case-in-any-language) above, and
`message` — or no `kind` at all — goes here.

1. Read the case. Take `input` as text (encode it as UTF-8 bytes), or `input_hex` as
   bytes, and hand it to the decoder for `wire`.
2. If `expect.decode` is `"error"`: the decoder must fail, and the failure's kind, path
   and field must equal `expect.error`. A decoder that succeeds here fails the case.
3. If `expect.decode` is `"ok"`: the decoder must succeed. Then
   - project the decoded message into the canonical model and compare with
     `message`;
   - compare `validate`'s findings with `violations` as sets of
     `{rule, severity, path}`;
   - compare `is_control` and the text extraction with `is_control` and `text`;
   - if `tokens` is present, compare the token classification, in order;
   - if `token_count` or `first_submessage_token` is present, compare each — both read
     the first submessage as §5 ¶2 does, which is what makes them different questions
     from `tokens`;
   - if `canonical_json` is present, re-encode and compare the string exactly.
4. Report the case name and the case kind with every difference. A corpus is only
   useful when a failure says which case and which field — and with two kinds in one
   tree, which question the difference is an answer to.

A case may carry expectations an implementation does not implement — `tokens` and
`canonical_json` are optional parts of an implementation as well as of a case. Skipping
one is a defensible choice; silently skipping a case because its `expect` has a key the
runner does not recognise is not. Fail on unknown keys instead. The same rule decides
what to do with a `kind` the runner does not know: report it as a failure, because a
case that was never run and a case that passed must never look alike.

An implementation with no clause-6 layer at all can skip the exchange cases as a group,
and should say how many it skipped. What it may not do is report the corpus as passed:
101 of 125 is a different claim from 125 of 125, and the twenty-four are where the
protocol stops being about a message and starts being about an exchange.

`tools/cbor_encode.py` regenerates the bytes of a CBOR case from a JSON description of
the frame, using only the Python standard library, so that a reader can check a case's
`input_hex` rather than trust it. Four markers stand in for what JSON cannot spell:
`{"$bytes": "<base64>"}` for a byte string, `{"$tag": [n, value]}` for a tagged value,
`{"$map": [[key, value], …]}` for a map whose keys need not be text, and
`{"$float": "inf"}` (also `"-inf"`, `"nan"`) for a non-finite double, which JSON has no
literal for either. `--prefix N` emits the first N bytes, which is how the truncated
frame of `ecma-432/08` was made. A case whose frame is two messages joined, such as
`ecma-432/14`, is the concatenation of two runs; its `source.note` says so.
