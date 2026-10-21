/**
 * How a JavaScript exception becomes a WIT error, and what happens where it cannot.
 *
 * jco turns a thrown value into a WIT error by one rule, and it is measured against
 * jco 1.39.0 rather than assumed: `getErrorPayload(e)` returns an **own `payload`**
 * property if there is one, **re-throws anything `instanceof Error`**, and otherwise
 * takes the thrown value as the payload. So a user's `throw new Error('…')` inside a
 * handler does *not* become `endpoint-error.internal` — it escapes the lowering and
 * rejects the whole exported call. Every one of the ten host functions therefore runs
 * inside a `try`, and every `catch` throws a {@link LoweredError} instead, which is an
 * `Error` (so it has a stack) with an own `payload` (so jco lowers it).
 *
 * **Five of the ten have no error channel at all**, and one more class beside them.
 * `host-authority`'s `is-own`, `issued` and `mint` are synchronous `func`s returning
 * plain values, because the contract requires them to be cheap; so are the four
 * configuration reads — `host-endpoint.name`, `.conformity`, `.rejection-policy`,
 * `.http-binding` — and `host-layers.count`. For all of them there is nothing to lower
 * into, so the exception is reported through the deployment's `onHostError` and the safe
 * default is answered, by {@link reported} below. That is a decision and not an
 * oversight: the alternative is an exception crossing back out through the component,
 * which fails the whole exchange for a token lookup that could have said "I do not know
 * this one" — or, for a configuration read, for a getter on the deployment's own object.
 *
 * The one exception to the exception is {@link NotInstalledError}, which is re-thrown
 * rather than defaulted: "the engine asked for an end-point and there is none" is a
 * programming error of the caller's and not a runtime failure of a deployment's, and a
 * default would hide it behind an exchange that looks answered.
 */

import { EndpointFailure, LoweredError, NlipError, TransportFailedError } from '../errors.js';
import { toWitEndpointError, toWitTransportError } from '../boundary.js';
import { deployment, reportToConsole } from '../registry.js';
import type * as wit from '../wasm/interfaces/nlipsy-engine-types.js';

/** No end-point has been installed and the component is asking one a question. */
export class NotInstalledError extends NlipError {
  constructor(what: string) {
    super(
      'not-installed',
      `nlipsy: the component asked the host for ${what}, and no end-point is installed. ` +
        'Call `createEndpoint({ … })` (or `serve({ … })`) before driving the engine, ' +
        'and keep the handle until you are done with it.',
    );
  }
}

/**
 * Run `read`; where it throws, report it and answer `fallback` instead.
 *
 * For the six imports the contract gives no error channel — the four configuration reads
 * and `host-layers.count` here, the authority's three answers in `authority.ts`, which
 * wrap themselves because each has a different safe answer. `fallback` is a function so
 * that the default is only computed on the path that needs it.
 *
 * A {@link NotInstalledError} is re-thrown: see this module's own doc.
 *
 * @throws whatever `read` threw, when `fallback` has nothing to answer with — a failure
 *         with no default is still better reported than replaced by a guess.
 */
export function reported<T>(where: string, read: () => T, fallback: () => T | undefined): T {
  try {
    return read();
  } catch (error) {
    if (error instanceof NotInstalledError) throw error;
    (deployment()?.onHostError ?? reportToConsole)(error, where);
    const instead = fallback();
    if (instead === undefined) throw error;
    return instead;
  }
}

/** The message a thrown value offers, without inventing one. */
export function messageOf(error: unknown): string {
  if (error instanceof Error && error.message !== '') return error.message;
  if (typeof error === 'string' && error !== '') return error;
  return String(error);
}

/**
 * Whatever a handler or a layer threw, as the contract's `endpoint-error`.
 *
 * An {@link EndpointFailure} carries the case the deployment chose — `unsupported`,
 * `unavailable` with its delay, `refused`, `timed-out` — and is used as it is. Anything
 * else is a bug rather than a decision, so it is `internal`, whose detail ECMA-434
 * §7.3.3 keeps away from the peer: `engine.public-message` is what the component shows
 * instead, and this package does not restate that rule.
 */
export function endpointErrorOf(error: unknown, where: string): wit.EndpointError {
  if (error instanceof EndpointFailure || (error as NlipError)?.code === 'endpoint') {
    return toWitEndpointError((error as EndpointFailure).error);
  }
  return { tag: 'internal', val: `${where}: ${messageOf(error)}` };
}

/** Throw it in the shape jco lowers. */
export function throwEndpointError(error: unknown, where: string): never {
  throw new LoweredError(endpointErrorOf(error, where), `${where}: ${messageOf(error)}`, {
    cause: error,
  });
}

/**
 * Whatever a transport threw, as the contract's `transport-error`.
 *
 * A {@link TransportFailedError} says which of the three it was and is used as it is.
 * Anything else is **`interrupted`**, and the choice is deliberate: the three cases
 * differ in what a caller may safely do next, `unreachable` promises "the end-point
 * cannot have seen this request", and a wrapper that promised that on behalf of a
 * transport it knows nothing about would be inviting a duplicate turn into somebody's
 * transcript. A transport that knows better says so: `unreachableError`, `interruptedError`
 * and `malformedTransportError` build the three.
 */
export function transportErrorOf(
  error: unknown,
  endpoint: string,
  where: string,
): wit.TransportError {
  if (error instanceof TransportFailedError || (error as NlipError)?.code === 'transport') {
    return toWitTransportError((error as TransportFailedError).error);
  }
  return {
    tag: 'interrupted',
    val: { endpoint, detail: `${where}: ${messageOf(error)}` },
  };
}

/** Throw it in the shape jco lowers. */
export function throwTransportError(error: unknown, endpoint: string, where: string): never {
  throw new LoweredError(
    transportErrorOf(error, endpoint, where),
    `${where}: ${messageOf(error)}`,
    { cause: error },
  );
}

/** A `result<_, string>`'s error, in the shape jco lowers. */
export function throwStringError(error: unknown, where: string): never {
  const detail = `${where}: ${messageOf(error)}`;
  throw new LoweredError(detail, detail, { cause: error });
}
