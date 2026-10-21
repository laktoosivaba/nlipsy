/**
 * The package's public vocabulary: `wit/nlipsy.wit` flattened.
 *
 * The contract is the truth about the interface and this file is the shape JavaScript
 * sees it in. Three differences, all of them jco's rendering rather than the contract's
 * meaning, and all of them stated once here so that nothing below has to restate them:
 *
 * 1. **Variants.** jco renders a WIT `variant` as `{ tag, val }`. That is a shape to
 *    lower through, not a shape to write code against: `outcome.val.val.retryAfterMs`
 *    says nothing about what it is. Every variant here is a discriminated union on a
 *    **`kind`** field with the payload flattened into it, so the failure a caller has to
 *    handle reads as one object. `boundary.ts` is the only place the two meet.
 * 2. **Numbers.** `u64` and `s64` cross as `bigint`. Milliseconds and byte counts are
 *    `number` here, because every one of them is a delay or a length and neither
 *    outlives `Number.MAX_SAFE_INTEGER`; `boundary.ts` converts. Two places keep a
 *    `bigint`: {@link Content}, where a JSON integer may be wider than a `number` can
 *    hold and rounding it would be losing the peer's data, and {@link Context.exchange},
 *    which is the contract's `u64` and whose `0n` is a *meaningful* falsy value.
 * 3. **Everything is a promise.** jco's output for this component returns a `Promise`
 *    from **every** export, the ones WIT declares as plain `func` included — measured,
 *    not assumed (README, "What jco actually renders"). So every function in this
 *    package that reaches the engine is `async`, `decode` and `encode` among them.
 *
 * Every item cites the clause it implements, as the WIT does. Nothing here is a rule:
 * these are the names of values the engine produces and consumes.
 */

// ---------------------------------------------------------------------------
// ECMA-430 clause 5 — the message model
// ---------------------------------------------------------------------------

/**
 * ECMA-430 Table 1, the closed set of formats.
 *
 * Closed in the contract too — a WIT `enum`, not a `string` — so a value the model
 * cannot hold cannot cross in either direction.
 */
export type Format = 'text' | 'token' | 'structured' | 'binary' | 'location' | 'generic';

/** Which encoding a document is in: ECMA-430 §5 ¶2's JSON, or ECMA-432 §7.1's CBOR. */
export type Wire = 'json' | 'cbor';

/**
 * A value {@link Content} can carry, at any depth.
 *
 * ECMA-404's values plus the two NLIP adds to them, and it is recursive because the
 * additions are not only top-level: a `Uint8Array` is ECMA-432 §7.1's byte string, and a
 * `bigint` is an integer wider than a `number` holds — which a peer may put one level
 * down as readily as at the top, and which this package keeps rather than rounds.
 */
export type ContentValue =
  | string
  | number
  | bigint
  | boolean
  | null
  | Uint8Array
  | ContentValue[]
  | { [key: string]: ContentValue };

/**
 * §5.1.4's content.
 *
 * A plain JavaScript value, which is the whole point of the wrapper: the boundary's
 * `variant content { null, %bool, number, text, bytes, structured }` is
 * `boundary.ts`'s business and never a caller's.
 *
 * - a `string` is `text`, a `boolean` is `%bool`, `null` is `null`;
 * - a `Uint8Array` is `bytes`, which JSON spells as base64 (§6.4) and CBOR as a byte
 *   string — the distinction a `{"$bytes": …}` marker exists to preserve;
 * - an array or an object is `structured`, carried across as its canonical JSON text
 *   (WIT has no recursive types). Object keys come back **sorted**, because the engine
 *   holds them in a `BTreeMap` and ECMA-404 objects are unordered;
 * - a `number` is a number, and a `bigint` is one too — a JSON integer wider than
 *   `Number.MAX_SAFE_INTEGER` arrives as a `bigint` rather than as a rounded `number`,
 *   **at the top level and nested**. The boundary reads the `structured` string with
 *   ES2025's JSON source access and writes a `bigint` back as `JSON.rawJSON` digits; a
 *   runtime with neither half throws and names this package rather than rounding.
 *
 * **What `JSON.stringify` does on the way out is what this type inherits.** The
 * `structured` string is that function's output, so a value JSON has no room for is
 * quietly reshaped rather than refused, and it is worth knowing which:
 * `new Map([['a', 1]])` and `new Set([1, 2])` both encode as `{}`; a `Date` encodes as
 * the ISO string its `toJSON` returns; `{ a: undefined, b: 1 }` loses `a`, and
 * `[undefined, 1]` becomes `[null, 1]`. None of those is this boundary's decision — they
 * are JSON's — but a caller writing `content: new Map(…)` will not read "canonical JSON
 * text" as "this sends `{}`". An object whose `toJSON` returns
 * nothing at all has no text to send and raises a `TypeError` that names the package,
 * and so does a bare `undefined`.
 */
export type Content = ContentValue;

/** §5.2 — one part of a message. */
export interface Submessage {
  /** An application's own name for this part. ECMA-430 defines none. */
  label?: string;
  /** Table 1's value. */
  format: Format;
  /** The format's refinement: a language for `text`, a media type for `binary`. */
  subformat: string;
  /** §5.1.4's value. */
  content: Content;
}

/**
 * §5.1 — a message.
 *
 * The first submessage is the message's own `format`, `subformat` and `content`; the
 * rest are in `submessages`, whose **order is significant** (§5.1.5).
 */
export interface Message {
  /** §5.1.1's reserved field. Absent on an ordinary exchange (TR/113 §11.1). */
  messageType?: string;
  /** §6.3's boolean. Absent is not `false`: the field was not written. */
  control?: boolean;
  /** An application's own name for the message. */
  label?: string;
  /** Table 1's value, for the first submessage. */
  format: Format;
  /** The first submessage's subformat. */
  subformat: string;
  /** The first submessage's content. */
  content: Content;
  /** The parts after the first, in document order. Absent and empty are the same. */
  submessages?: Submessage[];
}

// ---------------------------------------------------------------------------
// What was wrong with a document, and with a message
// ---------------------------------------------------------------------------

/**
 * The corpus's vocabulary of decode failures.
 *
 * `unmapped` is in the set because the engine's own error type is `#[non_exhaustive]`:
 * a kind added later is reported rather than folded into a neighbour.
 */
export type ParseErrorKind =
  | 'invalid-json'
  | 'cbor'
  | 'not-object'
  | 'missing-field'
  | 'wrong-type'
  | 'duplicate-field'
  | 'unknown-format'
  | 'base64'
  | 'unmapped';

/**
 * A document that is not a message.
 *
 * An implementation is not expected to reproduce `detail` — only `kind`, `path` and
 * `field`, which is what `conformance/cases/` compares.
 */
export interface ParseError {
  /** Which kind of failure. */
  kind: ParseErrorKind;
  /** `$` for the document or its first submessage, `submessages[i]` otherwise. */
  path: string;
  /** The canonical lower-case field name, for the kinds that name one. */
  field?: string;
  /** The prose, for a human. Asserted by nothing. */
  detail: string;
}

/** How serious a clause-5 finding is. */
export type Severity = 'error' | 'warning';

/** One clause-5 finding: a rule this message breaks, and where. */
export interface Violation {
  /** `error` — the message cannot be interpreted as written — or `warning`. */
  severity: Severity;
  /** Where in the message the finding is about. */
  path: string;
  /** The stable rule id, so a finding can be asserted without matching prose. */
  rule: string;
  /** The prose, for a human. */
  detail: string;
}

/** What a deployment does with a finding. */
export type Disposition = 'reject' | 'log' | 'ignore';

/** A finding and the disposition the deployment's policy gave it. */
export interface Finding {
  /** What clause 5 said. */
  violation: Violation;
  /** What this deployment does about it. */
  disposition: Disposition;
}

/** One direction's policy: what an error and a warning each do. */
export interface Policy {
  /** Default `reject`. */
  onError: Disposition;
  /** Default `log`. */
  onWarning: Disposition;
}

/** What a clause-5 finding does, per direction. */
export interface Rejection {
  /** Applied to the peer's request. */
  inbound: Policy;
  /** Applied to the end-point's own reply. */
  outbound: Policy;
}

/**
 * A request clause 6 will not answer.
 *
 * `violations` is the policy's verdict on the peer's message; `too-many-tokens` is
 * §6.2's ceiling, with the count the message carried (the first submessage included)
 * and the limit it exceeded — the contract's `token-ceiling` record, whose two fields
 * are named rather than positional.
 */
export type Rejected =
  | { kind: 'violations'; violations: Violation[] }
  | { kind: 'too-many-tokens'; count: number; limit: number };

// ---------------------------------------------------------------------------
// ECMA-430 clause 6 — the exchange
// ---------------------------------------------------------------------------

/** §5.3's two reserved token subformats. Every other subformat is the application's. */
export type TokenKind = 'conversation' | 'authentication';

/** The clause-6 switches. */
export interface Config {
  /** §6.2.1 — issue a `conversation` token and keep confirming it. */
  issueConversationToken: boolean;
  /** §6.2.2 — issue an `authentication` token. */
  issueAuthenticationToken: boolean;
  /** §6.2's ceiling on how many token submessages one message may carry. */
  maxTokensPerMessage: number;
}

/**
 * What a runtime knows about one exchange, and hands to every callback of it.
 *
 * The contract's `context`, **by value**: there is nothing to substitute and nothing to
 * leak. A host that keeps state about an exchange keys it
 * on {@link Context.exchange}.
 */
export interface Context {
  /** An opaque label for whoever is on the other end. */
  peer?: string;
  /** The identifier for this one request/response pair (`x-nlip-correlator`). */
  correlator?: string;
  /**
   * The number that names this exchange.
   *
   * `0n` means "the caller names no exchange" and the guest mints one for the call;
   * **it is falsy and it is meaningful**, so nothing here normalises it away. Any other
   * value travels unchanged to every callback of that exchange. It is a per-instance
   * counter, not a correlator, and it is never on a wire.
   */
  exchange: bigint;
}

/**
 * Why an end-point could not answer.
 *
 * `crates/nlipsy-core/src/endpoint.rs`'s five cases, with durations in milliseconds.
 * `internal`'s detail is for a log and not for a peer — ECMA-434 §7.3.3 — which is what
 * `publicMessage` is for.
 */
export type EndpointError =
  | { kind: 'unsupported'; detail: string }
  | { kind: 'unavailable'; detail: string; retryAfterMs?: number }
  | { kind: 'internal'; detail: string }
  | { kind: 'refused'; rejected: Rejected }
  | { kind: 'timed-out'; afterMs: number };

/** How one exchange ended. */
export type Outcome =
  | { kind: 'ok' }
  | { kind: 'malformed'; error: ParseError }
  | { kind: 'refused'; rejected: Rejected }
  | { kind: 'failed'; error: EndpointError };

/** What a refusal still owes the peer — §6.2's echo, §6.3's carry. */
export type Owed = 'nothing' | 'clause6' | 'control-only';

/** A reply that has been through clause 6's outbound half. */
export interface Finished {
  /** What goes back to the peer. */
  message: Message;
  /** What clause 5 said about the end-point's own answer. */
  findings: Finding[];
  /** `true` when the answer was refused and `message` is the replacement. */
  withheld: boolean;
}

/**
 * One exchange, answered.
 *
 * `findings` is one list: the request's first, then the reply's. The boundary between
 * the two halves is not recoverable from it — a caller that needs the direction drives
 * `corpus.check` and `corpus.finish` itself, which is what the corpus runner does.
 */
export interface Served {
  /** What goes back to the peer. A message on every outcome, refusals included. */
  reply: Message;
  /** How the exchange ended. */
  outcome: Outcome;
  /**
   * `true` when clause 5 rejected the end-point's own answer and `reply` is the
   * replacement. Only the binding's status line changes — `StatusMap.withheld`.
   *
   * It is what the finishing step reported about the end-point's **own** answer, and a
   * layer's `after` runs between that step and the wire: a layer that replaces a
   * withheld reply sends a good message at `StatusMap.withheld`, and one that breaks
   * clause 5 afterwards sends a bad one at `200` — the deployment's bug either way. The
   * sentence is the contract's, on `types.served.withheld` in `wit/nlipsy.wit`.
   */
  withheld: boolean;
  /** Every clause-5 finding the exchange reported, request then reply. */
  findings: Finding[];
}

// ---------------------------------------------------------------------------
// ECMA-431 §6 — what the binding in the guest is configured with
// ---------------------------------------------------------------------------

/**
 * Which status each way of failing becomes.
 *
 * No `ok`: every field here names a failure, and `withheld` is the one case where a
 * successful outcome is not a `200`. `tooLarge` is in the map and is **not** reachable
 * from an {@link Outcome} — the limit is enforced while the body is read, before there
 * are bytes to decode.
 *
 * It is *configuration*, not a value the wrapper reads to write a status line: the
 * status line is written inside the component.
 */
export interface StatusMap {
  malformed: number;
  refused: number;
  tooLarge: number;
  unsupported: number;
  unavailable: number;
  timedOut: number;
  internal: number;
  withheld: number;
  other: number;
}

/**
 * What a cross-origin caller is allowed, when a deployment allows one at all.
 *
 * The rule is the component's and is stated here only so that a reader configuring it
 * knows what they are turning on: **no cross-origin header at all** is emitted unless
 * the request carried an `Origin` and that origin matches.
 */
export interface Cors {
  /** Each a full origin (`https://example.com`), or a single `*`. */
  origins: string[];
  /** `Access-Control-Allow-Credentials: true`. Never beside `*`. */
  credentials: boolean;
}

/**
 * What the ECMA-431 §6 binding inside the component does.
 *
 * `defaultHttpOptions()` is the reference server's answer to all four, so a deployment
 * that changes nothing passes nothing.
 */
export interface HttpOptions {
  /** Largest request body read, in bytes. Enforced *while* the body is read. */
  maxRequestBodySize: number;
  /** Header the correlator travels in, or absent to carry none. */
  correlatorHeader?: string;
  /** Cross-origin access, or absent for an end-point no browser calls. */
  cors?: Cors;
  /** What each refusal looks like on the status line. */
  status: StatusMap;
}

// ---------------------------------------------------------------------------
// The client half
// ---------------------------------------------------------------------------

/** What the binding under a transport said about one exchange. */
export interface ExchangeDetail {
  kind: 'http';
  /** The status line. */
  status: number;
  /**
   * `Retry-After`, in milliseconds, when the end-point sent one it could read.
   *
   * `0` and `undefined` are different answers: `undefined` means the end-point said
   * nothing usable, and a caller that reads it as zero will retry immediately against
   * an end-point that never asked it to.
   */
  retryAfterMs?: number;
}

/** One exchange, as a transport carried it. */
export interface Exchange {
  /** The bytes that came back. */
  body: Uint8Array;
  /** What the binding said about them. */
  detail: ExchangeDetail;
}

/**
 * Why an exchange did not happen.
 *
 * `unreachable` and `interrupted` are not the same fact, and the difference is what a
 * retry policy turns on: a request that never left cannot have been recorded by the
 * end-point, and one that was cut off mid-reply may well have been.
 */
export type TransportError =
  | { kind: 'unreachable'; endpoint: string; detail: string }
  | { kind: 'interrupted'; endpoint: string; detail: string }
  | { kind: 'malformed'; detail: string };

/** A reply's body: a message, or bytes that were not one. */
export type Body =
  | { kind: 'message'; message: Message }
  | { kind: 'opaque'; text: string; why: string };

/**
 * What one `call` produced.
 *
 * Every derived view — `text`, `tokens`, `conversationTokens`, `authenticationTokens`,
 * `isError` — is computed by the engine and not by this wrapper, which is why they are
 * fields: they are rules, and the wrapper holds none (`wit/README.md`, *The design*).
 */
export interface Reply {
  /** What the binding said about the exchange. */
  detail: ExchangeDetail;
  /** How many bytes came back, before decoding. */
  bytes: number;
  /** The body, decoded if it was a message. */
  body: Body;
  /** Every `text` part, in document order, joined by single spaces (§5.1.5). */
  text?: string;
  /**
   * Every `token` submessage the reply carries, as it arrived.
   *
   * §6.2 ¶2 obliges an end-point that receives a token submessage to return "the same
   * submessage with the exact same format, subformat and content". A client is an
   * end-point and its next request is its response, so **this is what goes out on that
   * request** — before the caller removes the ones it created itself.
   */
  tokens: Submessage[];
  /**
   * The content of every `conversation` token the reply carries, in order.
   *
   * §6.2.1's identifier as a **key** — what a caller files a transcript under. It is
   * not wire material; {@link Reply.tokens} is.
   */
  conversationTokens: string[];
  /**
   * The content of every `authentication` token the reply carries, in order.
   *
   * §6.2.2 is the mandatory half of clause 6 and the obligation is this caller's, on
   * its next request. What discharges it is {@link Reply.tokens}; this is the key
   * beside it, for a caller that needs to notice a credential arriving.
   */
  authenticationTokens: string[];
  /** Whether the end-point marked the reply as an error in NLIP terms. */
  isError: boolean;
}

// ---------------------------------------------------------------------------
// What a host supplies — the five imports, as plain objects
// ---------------------------------------------------------------------------

/**
 * What a layer's way in answers with — the contract's `layer-step`.
 *
 * Build one with {@link pass} or {@link answer}; there is no shorthand, because "a bare
 * message means pass" would be a reading and this boundary carries two cases.
 *
 * @see Layer
 */
export type LayerStep =
  | { kind: 'pass'; request: Message }
  | { kind: 'answer'; reply: Message };

/** Carry on into the rest of the stack with this request. */
export function pass(request: Message): LayerStep {
  return { kind: 'pass', request };
}

/**
 * Answer here.
 *
 * The handler and every layer below are not run, and this layer's own `after` is not
 * called either. The layers *above* still unwind, and the engine still runs clause 6's
 * idempotent half over the result — which is the engine's business, not this package's
 * (`conformance/README.md`, "A short-circuited reply").
 */
export function answer(reply: Message): LayerStep {
  return { kind: 'answer', reply };
}

/**
 * One message-level layer: the way in, and the way out.
 *
 * A Rust layer is one "around" call that receives `next`. `next` cannot cross a
 * component boundary, so the contract splits it in two and **the engine composes them**:
 * outermost first on the way in, innermost first on the way out, with clause 6's
 * finishing step underneath all of them. Nothing in this package decides that order.
 *
 * Both halves are optional. A layer with only `after` passes every request down
 * untouched; a layer with only `before` returns every reply untouched.
 *
 * A token a layer *adds* to the request it passes down is echoed by the engine's
 * finishing step — §6.2 ¶2 is about the request the end-point saw — and the closing
 * step has no rule that would take it back, so a reply may carry a token neither
 * end-point created. ECMA-430 does not forbid it; the sentence is the contract's, on
 * `host-layers.before` in `wit/nlipsy.wit`, restated here because jco copies no docs.
 */
export interface Layer {
  /** A name for the log line a failure produces. Not a protocol value. */
  name?: string;
  /** The way in: {@link pass} the request down, or {@link answer} in its place. */
  before?(request: Message, cx: Context): LayerStep | Promise<LayerStep>;
  /**
   * The way out, innermost first — and **after** the finishing step, so a layer that
   * persists what §6.2.1 minted sees the reply that goes on the wire.
   */
  after?(reply: Message, cx: Context): Message | Promise<Message>;
}

/**
 * Who created a token, and what this end-point's own tokens are.
 *
 * All three run inside the request, on the path between the handler and the wire, and
 * the contract makes them **synchronous** for that reason: a deployment that needs a
 * database round trip mints ahead of time. All three are optional — an authority that
 * owns nothing and mints nothing is what a host with no session state supplies, and is
 * this package's default, because WIT has no default methods.
 *
 * **The contract gives these three no error channel**, so an exception thrown here
 * cannot be lowered into anything. This package catches it, answers the safe default
 * (`false`, `undefined`) and reports it through `onHostError`, rather than letting it
 * escape and fail the whole exchange.
 */
export interface TokenAuthority {
  /** `true` when this end-point created `token`, and may decline to echo it (§6.2). */
  isOwn?(token: string, cx: Context): boolean;
  /** The token of `kind` already issued for this conversation. A read, no side effect. */
  issued?(kind: TokenKind, cx: Context): string | undefined;
  /** Create a **new** token of `kind` and remember it (§6.2.1, §6.2.2). */
  mint?(kind: TokenKind, cx: Context): string | undefined;
}

/**
 * An alternative to `wasi:http/client` for the client half.
 *
 * For a host whose outgoing HTTP the runtime does not provide — a browser, and today
 * every runtime, since jco's `wasi:http` seam traps (README, "What does not run yet") —
 * and for a test. The bytes are the encoded request; where it is going is the
 * transport's business, which is why there is no URL in the signature.
 *
 * The correlator is not in the signature either: it travels in `cx` and the transport
 * decides how to write it, because it is one concept with a spelling per binding.
 */
export interface Transport {
  send(request: Uint8Array, cx: Context): Exchange | Promise<Exchange>;
}

/** Where a recording client puts what it saw: one exchange, already one JSON line. */
export interface RecordSink {
  write(line: string): void | Promise<void>;
}

/**
 * The deployment's handler and the four values it configures the engine with.
 *
 * `answer` returns a **message**, never a finished reply: finishing is the engine's, it
 * happens at the bottom of the composed layer stack because §6.2.1's token is minted
 * there, and there is no shape in this contract for a host to hand back a reply that
 * skipped it.
 */
export interface Endpoint {
  /** A name for logs and for a dispatcher. Not a protocol value. */
  name?: string;
  /** The optional halves of clause 6 this end-point turns on. Defaults where absent. */
  config?: Partial<Config>;
  /** What a clause-5 finding does, inbound and outbound. Defaults where absent. */
  rejection?: Rejection;
  /** What the ECMA-431 §6 binding does. Defaults where absent. */
  http?: Partial<HttpOptions>;
  /** The user's handler. */
  answer(request: Message, cx: Context): Message | Promise<Message>;
  /** The deployment's layers, outermost first. */
  layers?: Layer[];
  /** §6.2's exception, §6.2.1's confirmation, §6.2.1/§6.2.2's minting. */
  authority?: TokenAuthority;
  /**
   * Where an exception the contract has no channel for is reported.
   *
   * Three places produce one: the four configuration functions and `layers.length`,
   * which the contract declares as plain `func`s with no `result`; the three
   * {@link TokenAuthority} answers, for the same reason; and a `RecordSink.write` whose
   * failure the engine has already turned into a transport error. Everywhere else a
   * thrown value becomes an `EndpointError` and travels in the protocol.
   */
  onHostError?(error: unknown, where: string): void;
}
