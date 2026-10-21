/**
 * What this package throws, and what it keeps on the throw.
 *
 * "Errors are values, never traps" is a statement about the **component** and it holds:
 * `decode` returns a `result`, `serve` returns a `served` with an `outcome`, and peer
 * input cannot take the instance down. It stops at the JavaScript seam, because jco
 * lifts a `result`'s error by *throwing* it (`wit/README.md` §5) — so the wrapper
 * has a choice to make about each one, and this file is where it is written down:
 *
 * - {@link MalformedError}, {@link RefusedError}, {@link TransportFailedError} and
 *   {@link ReplayError} are the four the wrapper re-throws, because each is a failure of
 *   the call the caller just made and JavaScript's way of reporting one is `throw`. Each
 *   carries the contract's value — a `ParseError`, a `Rejected`, a `TransportError` — so
 *   nothing is lost to prose;
 * - the failure of an *exchange* is not thrown: {@link Served.outcome} is a value, as it
 *   is in Rust, because an end-point that refused has still produced a reply the peer is
 *   owed.
 *
 * The other direction is {@link LoweredError}, and it is the mechanism the five host
 * modules depend on. jco's `getErrorPayload` reads an **own `payload` property** first
 * and re-throws anything else that is `instanceof Error`; so a plain `Error` from a
 * user's handler escapes the lowering and takes the whole call with it, and an `Error`
 * *with* an own `payload` is lowered into the WIT error the contract declares. Measured
 * against jco 1.39.0, not assumed.
 *
 * Every class keeps `cause`, so a wrapped failure can be followed back to the throw
 * that started it.
 */

import type { EndpointError, ParseError, Rejected, TransportError } from './types.js';

/** Options every error here accepts, so that `cause` is never dropped. */
export interface NlipErrorOptions {
  /** The error this one was raised from. */
  cause?: unknown;
}

/**
 * The base of every error this package throws.
 *
 * `code` is a stable string a caller can branch on without `instanceof`, which matters
 * once a package is loaded twice — two copies of a class are two classes, and a bundler
 * that duplicated this module would quietly break every `instanceof` in a consumer.
 */
export class NlipError extends Error {
  /** A stable discriminator: `malformed`, `refused`, `transport`, … */
  readonly code: string;

  constructor(code: string, message: string, options: NlipErrorOptions = {}) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.code = code;
    this.name = new.target.name;
  }
}

/** Bytes that are not a message — ECMA-430 §5, as `decode` reports it. */
export class MalformedError extends NlipError {
  /** The kind, the path and the field the corpus compares. */
  readonly error: ParseError;

  constructor(error: ParseError, options: NlipErrorOptions = {}) {
    super('malformed', `malformed NLIP message: ${error.detail}`, options);
    this.error = error;
  }
}

/** A request clause 6 will not answer — what `check` returns as `err`. */
export class RefusedError extends NlipError {
  /** The refusal: the violations, or §6.2's ceiling with its two counts. */
  readonly rejected: Rejected;

  constructor(rejected: Rejected, options: NlipErrorOptions = {}) {
    super('refused', describeRejection(rejected), options);
    this.rejected = rejected;
  }
}

/**
 * An exchange that did not happen.
 *
 * Named `TransportFailedError` and not `TransportError`, because
 * {@link types.TransportError} is the *value* the contract carries and the package
 * exports both from one namespace. The value is on `.error`.
 */
export class TransportFailedError extends NlipError {
  /** Which of the three ways it failed, and against which end-point. */
  readonly error: TransportError;

  constructor(error: TransportError, options: NlipErrorOptions = {}) {
    super('transport', describeTransport(error), options);
    this.error = error;
  }
}

/**
 * A recording could not answer this request.
 *
 * The contract's `client.replay` reports prose rather than a value — it is the `Display`
 * of a replay error or of the transport error the replay refused with — so this carries
 * the sentence and nothing else.
 */
export class ReplayError extends NlipError {
  constructor(detail: string, options: NlipErrorOptions = {}) {
    super('replay', detail, options);
  }
}

/**
 * What a handler or a layer throws to fail an exchange in the protocol's own terms.
 *
 * A handler that throws anything else fails as `internal` with its message carried and
 * its `Error` kept on this side, which is right for a bug and wrong for a decision: an
 * end-point that is at capacity, that does not speak this subformat, or that timed out
 * is saying something the peer can act on, and ECMA-434 §7.3.3 asks that `internal`'s
 * detail reach nobody but a log. Throw this to say which.
 *
 * {@link unsupported}, {@link unavailable}, {@link refused} and {@link timedOut} build
 * one each.
 */
export class EndpointFailure extends NlipError {
  /** The typed failure, as the contract carries it. */
  readonly error: EndpointError;

  constructor(error: EndpointError, options: NlipErrorOptions = {}) {
    super('endpoint', describeEndpoint(error), options);
    this.error = error;
  }
}

/** The one end-point slot, or the one record sink, is taken — see the README. */
export class AlreadyInstalledError extends NlipError {
  constructor(what: string, detail: string) {
    super('already-installed', `nlipsy: ${what} is already installed. ${detail}`);
  }
}

/**
 * An error carrying the WIT value jco should lower.
 *
 * `getErrorPayload` checks for an **own** `payload` property before it checks
 * `instanceof Error`, so this is both a real `Error` — with a stack, for whoever is
 * reading the log — and a lowerable one. `payload` is non-enumerable so that it does not
 * appear in a `JSON.stringify` of the error.
 */
export class LoweredError extends Error {
  constructor(payload: unknown, message: string, options: NlipErrorOptions = {}) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.name = 'LoweredError';
    Object.defineProperty(this, 'payload', { value: payload, enumerable: false });
  }
}

// ---------------------------------------------------------------------------
// The four typed failures a handler can answer with
// ---------------------------------------------------------------------------

/** This end-point does not speak that — §6.1's refusal to interpret. */
export function unsupported(detail: string, cause?: unknown): EndpointFailure {
  return new EndpointFailure({ kind: 'unsupported', detail }, { cause });
}

/**
 * Not now — a queue that is full, a dependency that is down.
 *
 * `retryAfterMs` becomes a `Retry-After` header inside the component's own binding; the
 * rule that turns one into the other is the engine's and is not restated here.
 */
export function unavailable(
  detail: string,
  retryAfterMs?: number,
  cause?: unknown,
): EndpointFailure {
  return new EndpointFailure(
    retryAfterMs === undefined
      ? { kind: 'unavailable', detail }
      : { kind: 'unavailable', detail, retryAfterMs },
    { cause },
  );
}

/** The end-point's own refusal — a rate limiter, a policy, a frame it will not take. */
export function refused(rejected: Rejected, cause?: unknown): EndpointFailure {
  return new EndpointFailure({ kind: 'refused', rejected }, { cause });
}

/** The end-point ran out of the time a deployment gave it. */
export function timedOut(afterMs: number, cause?: unknown): EndpointFailure {
  return new EndpointFailure({ kind: 'timed-out', afterMs }, { cause });
}

// ---------------------------------------------------------------------------
// The three ways a transport fails, for a transport that knows which
// ---------------------------------------------------------------------------

/**
 * The connection was never established — repeating cannot duplicate anything.
 *
 * Throw this from a {@link types.Transport} only when that is *known*. A transport that
 * throws anything else is reported as {@link interruptedError}, because promising the
 * peer never saw the request is the promise that invites a duplicate turn into somebody's
 * transcript.
 */
export function unreachableError(endpoint: string, detail: string, cause?: unknown) {
  return new TransportFailedError({ kind: 'unreachable', endpoint, detail }, { cause });
}

/** The request went out and the reply never came back. Genuinely unknown. */
export function interruptedError(endpoint: string, detail: string, cause?: unknown) {
  return new TransportFailedError({ kind: 'interrupted', endpoint, detail }, { cause });
}

/** A reply arrived and could not be read as the binding's framing at all. */
export function malformedTransportError(detail: string, cause?: unknown) {
  return new TransportFailedError({ kind: 'malformed', detail }, { cause });
}

// ---------------------------------------------------------------------------
// One line each, for a message a human reads
// ---------------------------------------------------------------------------

function describeRejection(rejected: Rejected): string {
  if (rejected.kind === 'too-many-tokens') {
    return (
      `the request carries ${rejected.count} token submessages, over this ` +
      `end-point's limit of ${rejected.limit}`
    );
  }
  const rules = rejected.violations.map((violation) => violation.rule).join(', ');
  return `the request does not conform: ${rules}`;
}

function describeTransport(error: TransportError): string {
  switch (error.kind) {
    case 'unreachable':
      return `${error.endpoint} could not be reached: ${error.detail}`;
    case 'interrupted':
      return `the exchange with ${error.endpoint} was interrupted: ${error.detail}`;
    case 'malformed':
      return `the request could not be sent: ${error.detail}`;
  }
}

function describeEndpoint(error: EndpointError): string {
  switch (error.kind) {
    case 'unsupported':
      return `the end-point does not support this request: ${error.detail}`;
    case 'unavailable':
      return `the end-point is unavailable: ${error.detail}`;
    case 'internal':
      return `the end-point could not answer: ${error.detail}`;
    case 'refused':
      return describeRejection(error.rejected);
    case 'timed-out':
      return `the end-point did not answer within ${error.afterMs} ms`;
  }
}

/**
 * The payload jco threw, when it threw one.
 *
 * Every `result` in the contract is a `throw` on this side and the `.d.ts` shows none of
 * it (`wit/README.md` §5). jco's `ComponentError` carries the error value as a
 * non-enumerable own `payload`; anything else is a failure of this package or of the
 * runtime and is re-thrown untouched.
 */
export function payloadOf(error: unknown): unknown {
  if (typeof error === 'object' && error !== null && Object.hasOwn(error, 'payload')) {
    return (error as { payload: unknown }).payload;
  }
  return undefined;
}
