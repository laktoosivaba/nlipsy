/**
 * The engine's own defaults, and the four rules a host answering *outside* the binding
 * would otherwise re-derive.
 *
 * Both halves are here for one reason. A deployment that changes one status code should
 * not have to re-type the other eight, and a host that produces a refusal the composed
 * order never saw — a rate limiter that has already decoded the request — owes the peer
 * the same three obligations through the same echo and the same confirmation. Writing
 * either in JavaScript is writing a second copy of a rule that moves — an `EndpointError`
 * variant that gained a delay would be honoured by the Rust binding and silently dropped
 * here — which is why the contract exports the rule instead (`wit/nlipsy.wit`, `engine`).
 */

import { engine } from './glue.js';
import {
  fromWitConfig,
  fromWitHttpOptions,
  fromWitMessage,
  fromWitRejection,
  toWitContext,
  toWitMessage,
  toWitOutcome,
} from './boundary.js';
import type { Config, Context, HttpOptions, Message, Outcome, Owed, Rejection } from './types.js';

/** Issue nothing, and refuse above 32 token submessages. */
export async function defaultConfig(): Promise<Config> {
  return fromWitConfig(await engine.defaultConfig());
}

/** Reject an error, log a warning, in both directions. */
export async function defaultRejection(): Promise<Rejection> {
  return fromWitRejection(await engine.defaultRejection());
}

/**
 * The reference server's ECMA-431 §6 binding, option for option.
 *
 * Nine status codes, a body limit and a header name: the largest block of values a host
 * would otherwise re-type, and a host that re-typed them would be the second copy of the
 * clause's defaults.
 */
export async function defaultHttpOptions(): Promise<HttpOptions> {
  return fromWitHttpOptions(await engine.defaultHttpOptions());
}

/**
 * What clause 6 still owes the peer for this outcome.
 *
 * `ok` owes the same as most failures, and that is not an accident of the match: the
 * success path runs the same rule.
 */
export async function owedBy(outcome: Outcome): Promise<Owed> {
  return await engine.owedBy(toWitOutcome(outcome));
}

/**
 * The text the peer may be shown, or nothing when there is nothing to explain.
 *
 * `undefined` for `ok` alone: the reply *is* the answer there. The rule includes
 * ECMA-434 §7.3.3, which is why the detail of an `internal` fault is deliberately left
 * out of the sentence it returns.
 */
export async function publicMessage(outcome: Outcome): Promise<string | undefined> {
  return await engine.publicMessage(toWitOutcome(outcome));
}

/**
 * The delay a failing outcome asked a peer to wait, in milliseconds.
 *
 * `undefined` unless the outcome is a failure that named one. `0` and `undefined` are
 * different answers.
 */
export async function retryAfterOf(outcome: Outcome): Promise<number | undefined> {
  const delay = await engine.retryAfterOf(toWitOutcome(outcome));
  return delay === undefined ? undefined : Number(delay);
}

/**
 * The reply a refusal or a failure goes back as.
 *
 * An NLIP `messageType: "Error"` message carrying `why`, with what {@link owedBy} says
 * clause 6 still owes applied to it. `request` is absent exactly when `owed` is
 * `nothing`: there was no message to echo from. Nothing is minted.
 */
export async function errorReply(
  why: string,
  owed: Owed,
  request?: Message,
  cx?: Context,
): Promise<Message> {
  return fromWitMessage(
    await engine.errorReply(
      request === undefined ? undefined : toWitMessage(request),
      why,
      owed,
      toWitContext(cx),
    ),
  );
}
