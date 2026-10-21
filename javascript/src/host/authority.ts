/**
 * `nlipsy:engine/host-authority` — who created a token, and what this end-point's own
 * tokens are.
 *
 * The target of `--map 'nlipsy:engine/host-authority=../host/authority.js'`. All three
 * are **synchronous** and all three return a plain value: the contract makes them so
 * because they run inside the request, on the path between the handler and the wire, and
 * a deployment that needs a database round trip is expected to mint ahead of time.
 *
 * **They therefore have no error channel**, and that decides what happens when a user's
 * authority throws: there is no WIT error to lower into, so the exception is reported
 * through the deployment's `onHostError` and the *safe* answer is given — `false` for
 * "did this end-point create that token", `undefined` for "what has it issued" and for
 * "mint one". Measured, not assumed: a throw from `is-own` propagates out of
 * `engine.finish` as a plain JavaScript error and fails the whole exchange, which is a
 * high price for a lookup that could have said "I do not know this one". The instance
 * survives it either way.
 *
 * All three are optional on the user's object. An authority that owns nothing and mints
 * nothing is what a deployment with no session state supplies, and is the default here:
 * WIT has no optional imports and no default methods, so the wrapper supplies what the
 * Rust trait's defaults would.
 */

import { fromWitContext } from '../boundary.js';
import { deployment } from '../registry.js';
import { NotInstalledError } from './lower.js';
import type * as wit from '../wasm/interfaces/nlipsy-engine-types.js';

/** The installed deployment, or an error that says what to do about there being none. */
function installed(what: string) {
  const found = deployment();
  if (found === undefined) throw new NotInstalledError(what);
  return found;
}

/**
 * `true` when this end-point created `token` — §6.2's exception.
 *
 * Absent, the answer is `false`: an end-point that keeps no memory created nothing, so
 * every token it sees is the peer's and is echoed.
 */
export function isOwn(token: string, cx: wit.Context): boolean {
  const found = installed('a token ownership answer');
  if (found.authority.isOwn === undefined) return false;
  try {
    return found.authority.isOwn(token, fromWitContext(cx));
  } catch (error) {
    found.onHostError(error, 'the authority’s `isOwn`');
    return false;
  }
}

/**
 * The token of `kind` this end-point has **already** issued — a read, no side effect.
 *
 * Consulted on every reply, refusals included: §6.2.1 obliges an end-point that created
 * a conversation token to keep confirming it.
 */
export function issued(kind: wit.TokenKind, cx: wit.Context): string | undefined {
  const found = installed('an issued token');
  if (found.authority.issued === undefined) return undefined;
  try {
    return found.authority.issued(kind, fromWitContext(cx));
  } catch (error) {
    found.onHostError(error, 'the authority’s `issued`');
    return undefined;
  }
}

/**
 * Create a **new** token of `kind` and remember it — §6.2.1, §6.2.2.
 *
 * Called only when `issued` returned nothing *and* the request brought no token of that
 * kind this end-point did not create, so an implementation need not check either.
 * Returning nothing declines, which is always allowed: both clauses are optional.
 */
export function mint(kind: wit.TokenKind, cx: wit.Context): string | undefined {
  const found = installed('a minted token');
  if (found.authority.mint === undefined) return undefined;
  try {
    return found.authority.mint(kind, fromWitContext(cx));
  } catch (error) {
    found.onHostError(error, 'the authority’s `mint`');
    return undefined;
  }
}
