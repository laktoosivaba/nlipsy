/**
 * `nlipsy:engine/host-layers` — the deployment's message-level layers, as a before/after
 * pair per index.
 *
 * The target of `--map 'nlipsy:engine/host-layers=../host/layers.js'`. A Rust layer is
 * one "around" call that receives `next`; `next` cannot cross a component boundary, so
 * the contract addresses each layer by **index** and asks two questions instead of one.
 *
 * **The order is the engine's.** It composes these into exactly the `Middleware` stack
 * `nlipsy-core` runs — outermost first on the way in, innermost first on the way out,
 * clause 6's finishing step underneath all of them — and nothing in this file or
 * anywhere else in this package decides any of it. That is the whole point of the
 * design (`wit/README.md`, *The design*).
 *
 * `count()` answers `0` for a deployment with no layers, which is what a host that
 * supplies none says: WIT has no optional imports.
 */

import { fromWitContext, fromWitMessage, toWitLayerStep, toWitMessage } from '../boundary.js';
import { deployment } from '../registry.js';
import type { Layer } from '../types.js';
import { NotInstalledError, reported, throwEndpointError } from './lower.js';
import type * as wit from '../wasm/interfaces/nlipsy-engine-types.js';

/**
 * How many layers this deployment has. Read once per exchange.
 *
 * `func() -> u32`, so there is no error channel: a deployment whose `layers` cannot be
 * counted is reported through `onHostError` and answered `0`, which is a deployment with
 * none. Answering the exchange with no layers is a worse deployment than the one asked
 * for and a better one than an exchange nobody answered.
 */
export function count(): number {
  return reported(
    'the end-point’s `layers` count',
    () => {
      const found = deployment();
      if (found === undefined) throw new NotInstalledError('a layer count');
      return found.layers.length;
    },
    () => 0,
  );
}

/** The layer at `index`, or a failure naming what was asked for. */
function at(index: number): Layer {
  const found = deployment();
  if (found === undefined) throw new NotInstalledError('a layer');
  const layer = found.layers[index];
  if (layer === undefined) {
    throw new RangeError(
      `layer ${index} of ${found.layers.length}: the engine asked for a layer this ` +
        'end-point does not have',
    );
  }
  return layer;
}

/**
 * The way in.
 *
 * `pass` hands the request down; `answer` short-circuits — the handler and every layer
 * below are not run, this layer's own `after` is not called, and what the reply still
 * owes clause 6 is the engine's business. A layer with no `before` passes the request it
 * was given, and it does so **without converting it**: there is nothing for a layer that
 * does not exist to change, and a round trip through the public shape could only lose.
 */
export async function before(
  index: number,
  request: wit.Message,
  cx: wit.Context,
): Promise<wit.LayerStep> {
  try {
    const layer = at(index);
    if (layer.before === undefined) return { tag: 'pass', val: request };
    return toWitLayerStep(await layer.before(fromWitMessage(request), fromWitContext(cx)));
  } catch (error) {
    throwEndpointError(error, describe(index, 'before'));
  }
}

/**
 * The way out, innermost first — and **after** the finishing step, so a layer that
 * persists what §6.2.1 minted sees the reply that goes on the wire.
 *
 * Only reached when this layer's `before` answered `pass` *and* the stack below it
 * succeeded. A layer with no `after` returns the reply untouched, unconverted.
 */
export async function after(
  index: number,
  reply: wit.Message,
  cx: wit.Context,
): Promise<wit.Message> {
  try {
    const layer = at(index);
    if (layer.after === undefined) return reply;
    return toWitMessage(await layer.after(fromWitMessage(reply), fromWitContext(cx)));
  } catch (error) {
    throwEndpointError(error, describe(index, 'after'));
  }
}

/** How a lowered failure names the layer it came from. */
function describe(index: number, half: 'before' | 'after'): string {
  const found = deployment();
  const layer = found?.layers[index];
  const named = layer?.name === undefined ? '' : ` (${layer.name})`;
  return `layer ${index}${named}’s \`${half}\``;
}
