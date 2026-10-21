/**
 * Clause 6's two steps on their own, for a corpus runner.
 *
 * **Not part of the public API**, and the reason is in `wit/nlipsy.wit`: `serve` is the
 * order this engine runs, and a caller that composes `check` and `finish` itself is
 * composing a *second* order and owes §6.1's reply on every path. These are exported so
 * that `conformance/README.md`'s language-neutral algorithm can be driven step by step,
 * which is the only way to attribute a finding to `inbound` or `outbound` — `served`
 * carries one list with no marked boundary — and for nothing else. A deployment calls
 * `InstalledEndpoint.serve` or `serve()`.
 *
 * All three run against the installed deployment, so an end-point has to be installed
 * first; the authority a case scripts is that deployment's.
 */

import { engine } from './glue.js';
import { asRefused } from './lift.js';
import {
  fromWitFinding,
  fromWitFinished,
  fromWitServed,
  toWitContext,
  toWitMessage,
} from './boundary.js';
import type { Context, Finding, Finished, Message, Served } from './types.js';

/**
 * Clause 6's first step: clause 5 under the inbound policy, then §6.2's token ceiling.
 *
 * In that order, because a message that cannot be interpreted has no meaningful token
 * count. The findings it returns are the ones it did **not** reject.
 *
 * @throws RefusedError carrying the `rejected` — the violations, or the ceiling with its
 *         two counts.
 */
export async function check(request: Message, cx?: Context): Promise<Finding[]> {
  try {
    return (await engine.check(toWitMessage(request), toWitContext(cx))).map(fromWitFinding);
  } catch (error) {
    throw asRefused(error);
  }
}

/**
 * Clause 6's second step, over the end-point's own reply.
 *
 * Five steps in one order: the reply is sanitised against clause 5 *before* anything is
 * added to it, then §6.2's echo, then §6.2.1's confirmation, then the minting, then
 * §6.3's control carry. Minting is here and nowhere else, which is why the composed
 * order runs this at the bottom of the layer stack rather than above it.
 */
export async function finish(
  request: Message,
  reply: Message,
  cx?: Context,
): Promise<Finished> {
  return fromWitFinished(
    await engine.finish(toWitMessage(request), toWitMessage(reply), toWitContext(cx)),
  );
}

/**
 * The composed order over a message already decoded.
 *
 * What a `short_circuit` corpus case drives: the reply comes from a layer whose `before`
 * answers, so the handler and the finishing step are never reached and what the engine
 * reports is `check`'s findings alone.
 */
export async function serveMessage(request: Message, cx?: Context): Promise<Served> {
  return fromWitServed(await engine.serveMessage(toWitMessage(request), toWitContext(cx)));
}
