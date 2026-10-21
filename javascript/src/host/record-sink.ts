/**
 * `nlipsy:engine/host-record-sink` — where a recording client puts what it saw.
 *
 * The target of `--map 'nlipsy:engine/host-record-sink=../host/record-sink.js'`. One
 * line per exchange, already rendered: the spelling is the engine's
 * `Recorded::to_json_line`, which is what lets a recording made from JavaScript load in
 * a Rust test and the other way round. The sink needs no JSON codec of its own.
 *
 * **`write` carries no context**, so this is the one of the five that cannot be keyed at
 * all — see `registry.ts`. One sink per process, claimed and refused rather than
 * silently shared.
 *
 * A failure is reported and never swallowed: the engine turns it into
 * `transport-error.malformed`, because a recording that quietly stopped growing would
 * produce a fixture that passes for the wrong reason.
 */

import { sink } from '../registry.js';
import { LoweredError } from '../errors.js';
import { throwStringError } from './lower.js';

/** Append one exchange, already rendered as one JSON line. */
export async function write(line: string): Promise<void> {
  const slot = sink();
  if (slot === undefined) {
    const detail =
      'no record sink is installed; `client.call-recording` is reached only through a ' +
      'client built with `recordTo`';
    throw new LoweredError(detail, detail);
  }
  try {
    await slot.sink.write(line);
  } catch (error) {
    slot.onHostError(error, 'the record sink’s `write`');
    throwStringError(error, 'the record sink’s `write`');
  }
}
