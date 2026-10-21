/**
 * The contract's shapes and the package's, in both directions.
 *
 * `crates/nlipsy-component/src/convert.rs` is this file's mirror on the other side of
 * the boundary, and it is the same job for the same reason: `wit/nlipsy.wit` is a
 * projection of types that already exist, so almost everything here is a rename. What
 * is not a rename is named in `convert.rs`'s own module doc, and reappears here:
 *
 * 1. **`content.structured`.** WIT has no recursive types, so an array or an object
 *    crosses as the canonical JSON text of the value. This side writes that text with
 *    `JSON.stringify` and reads it with `JSON.parse`; the producer is always this file
 *    and never a caller, which is what keeps the string JSON. Object keys come back
 *    **sorted**,
 *    because the engine holds them in a `BTreeMap`.
 * 2. **Durations and counts.** Milliseconds as `bigint` on one side, `number` on the
 *    other. A delay that needed more than 53 bits would be 285 000 years.
 * 3. **Numbers.** ECMA-404's number space is three cases in the contract — `int(s64)`,
 *    `uint(u64)`, `float(f64)` — and one type in JavaScript. Coming *in*, an integer
 *    within `Number.MAX_SAFE_INTEGER` becomes a `number` and anything wider stays a
 *    `bigint`, so a peer's 64-bit identifier is not silently rounded. Going *out*, the
 *    tag is picked from the value, which has one consequence worth stating: a `number`
 *    that is integral goes out as `int`, so `3.0` written in JavaScript re-encodes as
 *    `3`. JavaScript has no way to tell the two apart, and `3` is the answer that
 *    surprises a caller least.
 *
 *    **One level down, that is `JSON.parse`'s and `JSON.stringify`'s job**, and both of
 *    them round — a wide integer inside `structured` content would come back rounded
 *    while the same integer at the top level crossed exactly. So neither is called
 *    plainly here. Reading, the reviver
 *    takes ES2025's source text and lifts an integer a `number` cannot hold into a
 *    `bigint` ({@link isWideIntegerSource}); writing, the replacer hands a `bigint` back
 *    as `JSON.rawJSON` digits. Where a runtime has neither half this throws and names
 *    the package rather than falling back, because the fallback is silent wrong data.
 *
 * 4. **`context.exchange` is a `bigint` whose `0n` means something.** `0` is the
 *    contract's "the caller names no exchange"; it is falsy, and nothing here
 *    normalises it. The same holds for `option<u64>` as `retryAfterMs`, where `some(0)`
 *    and `none` are different answers.
 *
 * Naming: `toWit` is package → contract, `fromWit` is contract → package, for every
 * item. Nothing here decides anything: every rule is the engine's.
 */

import type * as wit from './wasm/interfaces/nlipsy-engine-types.js';
import { NlipError } from './errors.js';
import type {
  Body,
  Config,
  Content,
  ContentValue,
  Context,
  Cors,
  EndpointError,
  Exchange,
  ExchangeDetail,
  Finding,
  Finished,
  Format,
  HttpOptions,
  LayerStep,
  Message,
  Outcome,
  ParseError,
  Policy,
  Rejected,
  Rejection,
  Reply,
  Served,
  StatusMap,
  Submessage,
  TransportError,
  Violation,
} from './types.js';

/** jco names the contract's `number` variant `Number`, which shadows the global. */
type WitNumber = wit.Number;

// ---------------------------------------------------------------------------
// Content — §5.1.4
// ---------------------------------------------------------------------------

/** The widest integer a JavaScript `number` holds exactly. */
const SAFE = BigInt(Number.MAX_SAFE_INTEGER);

/** The range `s64` covers. */
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
/** The range `u64` covers. */
const U64_MAX = 2n ** 64n - 1n;

function fromWitNumber(value: WitNumber): number | bigint {
  switch (value.tag) {
    case 'int':
    case 'uint':
      // Exact where a `number` is exact, and a `bigint` where it is not.
      return value.val <= SAFE && value.val >= -SAFE ? Number(value.val) : value.val;
    case 'float':
      return value.val;
  }
}

function toWitNumber(value: number | bigint): WitNumber {
  if (typeof value === 'bigint') {
    if (value >= I64_MIN && value <= I64_MAX) return { tag: 'int', val: value };
    if (value >= 0n && value <= U64_MAX) return { tag: 'uint', val: value };
    // Wider than the contract's two integer cases. `f64` is the only remaining total
    // answer, and it is the one ECMA-404 gives a number that large anyway.
    return { tag: 'float', val: Number(value) };
  }
  if (Number.isInteger(value) && Math.abs(value) <= Number.MAX_SAFE_INTEGER) {
    return { tag: 'int', val: BigInt(value) };
  }
  if (!Number.isFinite(value)) {
    // ECMA-432 §7.1 refuses NaN and the infinities at the decoder and the model has no
    // room for one, so this cannot be carried. Throwing here is the only answer that
    // does not invent a value: `0` and `null` are both lies about what was written.
    throw new TypeError(
      `content carries ${String(value)}, which ECMA-430's number space does not hold`,
    );
  }
  return { tag: 'float', val: value };
}

// ---------------------------------------------------------------------------
// A wide integer one level down
// ---------------------------------------------------------------------------

/**
 * Whether a number `JSON.parse` produced was *written* as an integer no `number` holds.
 *
 * ES2025 gives a reviver the source text of the value it is lifting, which is the only
 * way to tell `18446744073709551615` from the `18446744073709552000` a double rounds it
 * to. The three digits of the test are the three facts: the runtime produced a number,
 * the number is not exact, and the text it came from was an integer rather than a float
 * that is simply large.
 *
 * `test/corpus-runner.mjs`'s `parseCase` reads the corpus's own `u64::MAX` through this
 * same function — one copy of the predicate, which is why it is exported.
 */
export function isWideIntegerSource(value: unknown, context: unknown): boolean {
  if (typeof value !== 'number' || Number.isSafeInteger(value)) return false;
  const source = (context as { source?: unknown } | undefined)?.source;
  return typeof source === 'string' && /^-?[0-9]+$/.test(source);
}

/** A `JSON.parse` reviver that keeps a wide integer as a `bigint`. */
function reviveExactly(this: unknown, _key: string, value: unknown, context?: unknown): unknown {
  if (typeof value !== 'number' || Number.isSafeInteger(value)) return value;
  if (isWideIntegerSource(value, context)) {
    return BigInt((context as { source: string }).source);
  }
  const source = (context as { source?: unknown } | undefined)?.source;
  // The runtime gave the source and it was not an integer: a float, and a float is what
  // a `number` is for.
  if (typeof source === 'string') return value;
  // It gave none, so an integer that has already been rounded cannot be told from a
  // float that was always one. Wrong data is worse than no data.
  if (Number.isInteger(value)) {
    throw new NlipError(
      'unsupported-runtime',
      'nlipsy: this runtime does not give JSON.parse\'s reviver the source text, so an ' +
        `integer inside structured content (${String(value)}) cannot be read exactly`,
    );
  }
  return value;
}

/** A `bigint` as JSON digits rather than as `JSON.stringify`'s `TypeError`. */
function rawInteger(value: bigint): unknown {
  const raw = (JSON as { rawJSON?: (text: string) => unknown }).rawJSON;
  if (typeof raw !== 'function') {
    throw new NlipError(
      'unsupported-runtime',
      'nlipsy: this runtime has no JSON.rawJSON, so a bigint inside structured content ' +
        `(${String(value)}) cannot be written without rounding it`,
    );
  }
  return raw(String(value));
}

/**
 * A `Uint8Array` or a `bigint` nested inside structured content, as JSON spells one.
 *
 * §6.4 gives a `binary` content base64 and nothing else, so a byte string inside an
 * array or an object has no spelling of its own — a loss the contract's `content` type
 * records, and which JSON has with or without this boundary. `JSON.stringify` would
 * otherwise write `{"0":1,"1":2}`, which is not a loss but a corruption. A `bigint` it
 * refuses outright, with a `TypeError` that names neither the package nor the field; the
 * digits are what the engine reads on the other side.
 */
function structuredOut(this: unknown, _key: string, value: unknown): unknown {
  // `this[key]` is not consulted, because `JSON.stringify` has already called `toJSON` —
  // and neither a `Uint8Array` nor a `bigint` has one, so the two agree.
  if (value instanceof Uint8Array) return toBase64(value);
  if (typeof value === 'bigint') return rawInteger(value);
  return value;
}

/** §5.1.4's value, contract to package. */
export function fromWitContent(content: wit.Content): Content {
  switch (content.tag) {
    case 'null':
      return null;
    case 'bool':
      return content.val;
    case 'number':
      return fromWitNumber(content.val);
    case 'text':
      return content.val;
    case 'bytes':
      return content.val;
    case 'structured':
      // The producer of this string is the guest — `Content::to_json_value().to_string()`
      // — so it is JSON by construction. A string that is not is a host bug on the other
      // side; taking it as text is what the contract defines and what invents nothing.
      try {
        return JSON.parse(content.val, reviveExactly) as ContentValue;
      } catch (error) {
        // The fallback is for a string that is not JSON, and for nothing else: a runtime
        // that cannot read the number exactly has to say so rather than hand back the
        // whole document as text.
        if (error instanceof NlipError) throw error;
        return content.val;
      }
  }
}

/** §5.1.4's value, package to contract. */
export function toWitContent(content: Content): wit.Content {
  if (content === null) return { tag: 'null' };
  if (content instanceof Uint8Array) return { tag: 'bytes', val: content };
  switch (typeof content) {
    case 'string':
      return { tag: 'text', val: content };
    case 'boolean':
      return { tag: 'bool', val: content };
    case 'number':
    case 'bigint':
      return { tag: 'number', val: toWitNumber(content) };
    case 'object': {
      // The one recursive pair, flattened. `JSON.stringify` is the same text
      // `serde_json` writes on the other side, which is the invariant the contract's
      // `content` type states — and a nested byte string is base64 in it, because
      // JSON has no other spelling.
      const text = JSON.stringify(content, structuredOut);
      if (text === undefined) {
        // `JSON.stringify` answers `undefined` for a value it has no text for — an
        // object whose `toJSON` returns nothing is the reachable case. Lowering that
        // would raise jco's own `expected a string, received [undefined]`, which names
        // neither this package nor the value.
        throw new TypeError(
          'nlipsy: content has no JSON text — `JSON.stringify` returned undefined, ' +
            'which is what a `toJSON` that answers nothing produces',
        );
      }
      return { tag: 'structured', val: text };
    }
    default:
      // `undefined` lands here, and that is the point: an absent content is not a
      // content, and lowering one would raise jco's own error about a missing variant
      // rather than this package's about the field.
      throw new TypeError(
        `content of type ${typeof content} is not a value ECMA-430 §5.1.4 can carry`,
      );
  }
}

/**
 * Standard base64 with padding, which is the spelling the engine writes.
 *
 * **The one rule this file holds**, and it is not delegable: WIT has no recursive types,
 * so the `structured` string is written *here* and a byte string nested in it has to be
 * spelled before it crosses. `Content::to_json_value` writes the same text on the other
 * side with `serde_json`, so the two must agree character for character or the round
 * trip breaks. What keeps that honest is a test rather than a comment:
 * `test/spec/boundary.mjs` "the nested base64 is the engine's own spelling" encodes a
 * `binary` message on the JSON wire, reads the base64 the **engine** wrote out of the
 * document, and compares it to this function over the same bytes — three padding lengths
 * and a byte over 0x7f.
 */
export function toBase64(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

// ---------------------------------------------------------------------------
// The message model — §5.1, §5.2
// ---------------------------------------------------------------------------

/** §5.2, contract to package. */
export function fromWitSubmessage(submessage: wit.Submessage): Submessage {
  const out: Submessage = {
    format: submessage.format as Format,
    subformat: submessage.subformat,
    content: fromWitContent(submessage.content),
  };
  if (submessage.label !== undefined) out.label = submessage.label;
  return out;
}

/** §5.2, package to contract. */
export function toWitSubmessage(submessage: Submessage): wit.Submessage {
  return {
    label: submessage.label,
    format: submessage.format,
    subformat: submessage.subformat,
    content: toWitContent(submessage.content),
  };
}

/**
 * §5.1, contract to package.
 *
 * An absent optional is an absent **key** and not a `null`: `content: null` is a value
 * Annex A allows and an absent field is not the same thing.
 */
export function fromWitMessage(message: wit.Message): Message {
  const out: Message = {
    format: message.format as Format,
    subformat: message.subformat,
    content: fromWitContent(message.content),
  };
  if (message.messageType !== undefined) out.messageType = message.messageType;
  if (message.control !== undefined) out.control = message.control;
  if (message.label !== undefined) out.label = message.label;
  if (message.submessages.length > 0) {
    out.submessages = message.submessages.map(fromWitSubmessage);
  }
  return out;
}

/** §5.1, package to contract. */
export function toWitMessage(message: Message): wit.Message {
  return {
    messageType: message.messageType,
    control: message.control,
    label: message.label,
    format: message.format,
    subformat: message.subformat,
    content: toWitContent(message.content),
    submessages: (message.submessages ?? []).map(toWitSubmessage),
  };
}

// ---------------------------------------------------------------------------
// The context — by value, in both directions
// ---------------------------------------------------------------------------

/** What every callback of one exchange is handed, contract to package. */
export function fromWitContext(cx: wit.Context): Context {
  const out: Context = { exchange: cx.exchange };
  if (cx.peer !== undefined) out.peer = cx.peer;
  if (cx.correlator !== undefined) out.correlator = cx.correlator;
  return out;
}

/**
 * The same, package to contract.
 *
 * `exchange` defaults to `0n`, which is the contract's "the caller names no exchange"
 * and makes the guest mint one for the call. It is **not** normalised away when the
 * caller did pass `0n` deliberately: `0n` *is* that request.
 */
export function toWitContext(cx: Context | undefined): wit.Context {
  return {
    peer: cx?.peer,
    correlator: cx?.correlator,
    exchange: cx?.exchange ?? 0n,
  };
}

// ---------------------------------------------------------------------------
// Findings, refusals, failures
// ---------------------------------------------------------------------------

/** A decode failure, in the three fields the corpus compares plus the prose. */
export function fromWitParseError(error: wit.ParseError): ParseError {
  const out: ParseError = { kind: error.kind, path: error.path, detail: error.detail };
  if (error.field !== undefined) out.field = error.field;
  return out;
}

/** One clause-5 finding. */
export function fromWitViolation(violation: wit.Violation): Violation {
  return {
    severity: violation.severity,
    path: violation.path,
    rule: violation.rule,
    detail: violation.detail,
  };
}

/** A finding and what this deployment does about it. */
export function fromWitFinding(finding: wit.Finding): Finding {
  return {
    violation: fromWitViolation(finding.violation),
    disposition: finding.disposition,
  };
}

/** A request clause 6 will not answer. */
export function fromWitRejected(rejected: wit.Rejected): Rejected {
  if (rejected.tag === 'too-many-tokens') {
    return { kind: 'too-many-tokens', count: rejected.val.count, limit: rejected.val.limit };
  }
  return { kind: 'violations', violations: rejected.val.map(fromWitViolation) };
}

/** The same, the other way — what a host's own refusal hands the engine. */
export function toWitRejected(rejected: Rejected): wit.Rejected {
  if (rejected.kind === 'too-many-tokens') {
    return { tag: 'too-many-tokens', val: { count: rejected.count, limit: rejected.limit } };
  }
  return {
    tag: 'violations',
    val: rejected.violations.map((violation) => ({
      severity: violation.severity,
      path: violation.path,
      rule: violation.rule,
      detail: violation.detail,
    })),
  };
}

/** Why an end-point could not answer, contract to package. */
export function fromWitEndpointError(error: wit.EndpointError): EndpointError {
  switch (error.tag) {
    case 'unsupported':
      return { kind: 'unsupported', detail: error.val };
    case 'unavailable': {
      const out: EndpointError = { kind: 'unavailable', detail: error.val.detail };
      // `some(0)` and `none` are different answers and a falsy check would lose the
      // distinction, which is the whole reason this is written out.
      if (error.val.retryAfterMs !== undefined) {
        out.retryAfterMs = Number(error.val.retryAfterMs);
      }
      return out;
    }
    case 'internal':
      return { kind: 'internal', detail: error.val };
    case 'refused':
      return { kind: 'refused', rejected: fromWitRejected(error.val) };
    case 'timed-out':
      return { kind: 'timed-out', afterMs: Number(error.val) };
  }
}

/** Why an end-point could not answer, package to contract. */
export function toWitEndpointError(error: EndpointError): wit.EndpointError {
  switch (error.kind) {
    case 'unsupported':
      return { tag: 'unsupported', val: error.detail };
    case 'unavailable':
      return {
        tag: 'unavailable',
        val: {
          detail: error.detail,
          retryAfterMs: error.retryAfterMs === undefined ? undefined : BigInt(error.retryAfterMs),
        },
      };
    case 'internal':
      return { tag: 'internal', val: error.detail };
    case 'refused':
      return { tag: 'refused', val: toWitRejected(error.rejected) };
    case 'timed-out':
      return { tag: 'timed-out', val: BigInt(error.afterMs) };
  }
}

/** How one exchange ended, contract to package. */
export function fromWitOutcome(outcome: wit.Outcome): Outcome {
  switch (outcome.tag) {
    case 'ok':
      return { kind: 'ok' };
    case 'malformed':
      return { kind: 'malformed', error: fromWitParseError(outcome.val) };
    case 'refused':
      return { kind: 'refused', rejected: fromWitRejected(outcome.val) };
    case 'failed':
      return { kind: 'failed', error: fromWitEndpointError(outcome.val) };
  }
}

/** How one exchange ended, package to contract — what `owedBy` and the rules read. */
export function toWitOutcome(outcome: Outcome): wit.Outcome {
  switch (outcome.kind) {
    case 'ok':
      return { tag: 'ok' };
    case 'malformed':
      return {
        tag: 'malformed',
        val: {
          kind: outcome.error.kind,
          path: outcome.error.path,
          field: outcome.error.field,
          detail: outcome.error.detail,
        },
      };
    case 'refused':
      return { tag: 'refused', val: toWitRejected(outcome.rejected) };
    case 'failed':
      return { tag: 'failed', val: toWitEndpointError(outcome.error) };
  }
}

/** A reply that has been through clause 6's outbound half. */
export function fromWitFinished(finished: wit.Finished): Finished {
  return {
    message: fromWitMessage(finished.message),
    findings: finished.findings.map(fromWitFinding),
    withheld: finished.withheld,
  };
}

/** One exchange, answered. */
export function fromWitServed(served: wit.Served): Served {
  return {
    reply: fromWitMessage(served.reply),
    outcome: fromWitOutcome(served.outcome),
    withheld: served.withheld,
    findings: served.findings.map(fromWitFinding),
  };
}

/** What a layer's way in answered with, package to contract. */
export function toWitLayerStep(step: LayerStep): wit.LayerStep {
  return step.kind === 'answer'
    ? { tag: 'answer', val: toWitMessage(step.reply) }
    : { tag: 'pass', val: toWitMessage(step.request) };
}

// ---------------------------------------------------------------------------
// Clause 6's configuration, and the binding's
// ---------------------------------------------------------------------------

/** The clause-6 switches, contract to package. */
export function fromWitConfig(config: wit.Config): Config {
  return {
    issueConversationToken: config.issueConversationToken,
    issueAuthenticationToken: config.issueAuthenticationToken,
    maxTokensPerMessage: config.maxTokensPerMessage,
  };
}

/** The clause-6 switches, package to contract. */
export function toWitConfig(config: Config): wit.Config {
  return {
    issueConversationToken: config.issueConversationToken,
    issueAuthenticationToken: config.issueAuthenticationToken,
    maxTokensPerMessage: config.maxTokensPerMessage,
  };
}

function fromWitPolicy(policy: wit.Policy): Policy {
  return { onError: policy.onError, onWarning: policy.onWarning };
}

/** What a clause-5 finding does, contract to package. */
export function fromWitRejection(rejection: wit.Rejection): Rejection {
  return {
    inbound: fromWitPolicy(rejection.inbound),
    outbound: fromWitPolicy(rejection.outbound),
  };
}

/** What a clause-5 finding does, package to contract. */
export function toWitRejection(rejection: Rejection): wit.Rejection {
  return {
    inbound: { onError: rejection.inbound.onError, onWarning: rejection.inbound.onWarning },
    outbound: { onError: rejection.outbound.onError, onWarning: rejection.outbound.onWarning },
  };
}

/** Which status each way of failing becomes, contract to package. */
export function fromWitStatusMap(map: wit.StatusMap): StatusMap {
  return {
    malformed: map.malformed,
    refused: map.refused,
    tooLarge: map.tooLarge,
    unsupported: map.unsupported,
    unavailable: map.unavailable,
    timedOut: map.timedOut,
    internal: map.internal,
    withheld: map.withheld,
    other: map.other,
  };
}

/** Which status each way of failing becomes, package to contract. */
export function toWitStatusMap(map: StatusMap): wit.StatusMap {
  return {
    malformed: map.malformed,
    refused: map.refused,
    tooLarge: map.tooLarge,
    unsupported: map.unsupported,
    unavailable: map.unavailable,
    timedOut: map.timedOut,
    internal: map.internal,
    withheld: map.withheld,
    other: map.other,
  };
}

function fromWitCors(cors: wit.Cors): Cors {
  return { origins: [...cors.origins], credentials: cors.credentials };
}

/** The binding's options, contract to package. */
export function fromWitHttpOptions(options: wit.HttpOptions): HttpOptions {
  const out: HttpOptions = {
    maxRequestBodySize: options.maxRequestBodySize,
    status: fromWitStatusMap(options.status),
  };
  if (options.correlatorHeader !== undefined) out.correlatorHeader = options.correlatorHeader;
  if (options.cors !== undefined) out.cors = fromWitCors(options.cors);
  return out;
}

/** The binding's options, package to contract. */
export function toWitHttpOptions(options: HttpOptions): wit.HttpOptions {
  return {
    maxRequestBodySize: options.maxRequestBodySize,
    correlatorHeader: options.correlatorHeader,
    cors:
      options.cors === undefined
        ? undefined
        : { origins: [...options.cors.origins], credentials: options.cors.credentials },
    status: toWitStatusMap(options.status),
  };
}

// ---------------------------------------------------------------------------
// The client half
// ---------------------------------------------------------------------------

/** What a binding said about one exchange, contract to package. */
export function fromWitExchangeDetail(detail: wit.ExchangeDetail): ExchangeDetail {
  const out: ExchangeDetail = { kind: 'http', status: detail.val.status };
  if (detail.val.retryAfterMs !== undefined) {
    out.retryAfterMs = Number(detail.val.retryAfterMs);
  }
  return out;
}

/** What a binding said about one exchange, package to contract. */
export function toWitExchangeDetail(detail: ExchangeDetail): wit.ExchangeDetail {
  return {
    tag: 'http',
    val: {
      status: detail.status,
      retryAfterMs: detail.retryAfterMs === undefined ? undefined : BigInt(detail.retryAfterMs),
    },
  };
}

/** One exchange, package to contract — what a host transport hands back. */
export function toWitExchange(exchange: Exchange): wit.Exchange {
  return { body: exchange.body, detail: toWitExchangeDetail(exchange.detail) };
}

/** One exchange, contract to package. */
export function fromWitExchange(exchange: wit.Exchange): Exchange {
  return { body: exchange.body, detail: fromWitExchangeDetail(exchange.detail) };
}

/** Why an exchange did not happen, package to contract. */
export function toWitTransportError(error: TransportError): wit.TransportError {
  switch (error.kind) {
    case 'unreachable':
      return { tag: 'unreachable', val: { endpoint: error.endpoint, detail: error.detail } };
    case 'interrupted':
      return { tag: 'interrupted', val: { endpoint: error.endpoint, detail: error.detail } };
    case 'malformed':
      return { tag: 'malformed', val: error.detail };
  }
}

/** Why an exchange did not happen, contract to package. */
export function fromWitTransportError(error: wit.TransportError): TransportError {
  switch (error.tag) {
    case 'unreachable':
      return { kind: 'unreachable', endpoint: error.val.endpoint, detail: error.val.detail };
    case 'interrupted':
      return { kind: 'interrupted', endpoint: error.val.endpoint, detail: error.val.detail };
    case 'malformed':
      return { kind: 'malformed', detail: error.val };
  }
}

function fromWitBody(body: wit.Body): Body {
  if (body.tag === 'message') return { kind: 'message', message: fromWitMessage(body.val) };
  return { kind: 'opaque', text: body.val.text, why: body.val.why };
}

/** What one `call` produced, with the engine's five derived views carried through. */
export function fromWitReply(reply: wit.Reply): Reply {
  const out: Reply = {
    detail: fromWitExchangeDetail(reply.detail),
    bytes: reply.bytes,
    body: fromWitBody(reply.body),
    tokens: reply.tokens.map(fromWitSubmessage),
    conversationTokens: [...reply.conversationTokens],
    authenticationTokens: [...reply.authenticationTokens],
    isError: reply.isError,
  };
  if (reply.text !== undefined) out.text = reply.text;
  return out;
}
