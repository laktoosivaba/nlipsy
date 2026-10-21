/**
 * The other half of `host/lower.ts`: reading what jco threw.
 *
 * Every `result` in the contract arrives on this side as a **throw** — jco's
 * `ComponentError`, carrying the error value as a non-enumerable own `payload` — and the
 * generated `.d.ts` shows none of it. So every call into the engine that can fail is
 * wrapped, and each of the four failure shapes becomes the package's own error with the
 * contract's value on it. Anything without a payload is not the component's answer at
 * all and is re-thrown untouched, so a bug in this package or in the runtime does not
 * get dressed up as a protocol failure.
 */

import {
  MalformedError,
  RefusedError,
  ReplayError,
  TransportFailedError,
  payloadOf,
} from './errors.js';
import { fromWitParseError, fromWitRejected, fromWitTransportError } from './boundary.js';
import type * as wit from './wasm/interfaces/nlipsy-engine-types.js';

/** `protocol.decode`'s error: a structured `parse-error`. */
export function asMalformed(error: unknown): unknown {
  const payload = payloadOf(error);
  if (payload === undefined) return error;
  return new MalformedError(fromWitParseError(payload as wit.ParseError), { cause: error });
}

/** `engine.check`'s error: a `rejected`. */
export function asRefused(error: unknown): unknown {
  const payload = payloadOf(error);
  if (payload === undefined) return error;
  return new RefusedError(fromWitRejected(payload as wit.Rejected), { cause: error });
}

/** `client.call`, `call-through` and `call-recording`'s error: a `transport-error`. */
export function asTransportFailed(error: unknown): unknown {
  const payload = payloadOf(error);
  if (payload === undefined) return error;
  return new TransportFailedError(fromWitTransportError(payload as wit.TransportError), {
    cause: error,
  });
}

/** `client.replay`'s error: prose, because a replay failure is a sentence. */
export function asReplayFailed(error: unknown): unknown {
  const payload = payloadOf(error);
  if (payload === undefined) return error;
  return new ReplayError(String(payload), { cause: error });
}
