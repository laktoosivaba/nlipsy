/**
 * Which of the user's objects a host callback belongs to.
 *
 * This is the one piece of real design in the package, and it exists because of how jco
 * binds an import: `--map 'nlipsy:engine/host-endpoint=../host/endpoint.js'` names a
 * **module specifier**, and a module is a singleton. The five files under `src/host/`
 * are therefore not "the user's end-point" — they are five fixed sets of functions the
 * component calls, and something has to say *whose* end-point, *whose* layers and
 * *whose* transport a given call is about. That is this file.
 *
 * The answer is not one mechanism but two, and the contract is what splits them.
 *
 * **1. The deployment is a singleton, because the contract says so.** Five of the
 * imported functions carry no `context` at all — `host-endpoint.name`, `.conformity`,
 * `.rejection-policy`, `.http-binding` and `host-layers.count` are `func()` — so there
 * is nothing to key them on even in principle, and `wit/nlipsy.wit` says why in as many
 * words: *"There is one end-point per component instance: a deployment that answers two
 * kinds of request routes inside `answer`."* So `installDeployment` claims **one** slot
 * and refuses a second rather than letting two end-points take turns being whichever one
 * the component asked last. The handler, the layers and the authority travel together in
 * that slot, because they are one deployment's three answers and the component reads
 * them as one.
 *
 * **2. The transport is keyed on `cx.exchange`, because the wrapper mints it.** The
 * exchange number is `0` for "the caller names no exchange" and the guest mints one;
 * *any other value is the caller's and travels unchanged to every callback*
 * (`wit/nlipsy.wit`, `types.context`). Every call that reaches `host-transport.send`
 * starts at `client.call-through`, which **this package** invokes — so it passes a number
 * of its own minting, `host-transport.send` is handed that same number back, and a
 * process with five clients tells them apart exactly. Nothing is guessed and nothing is
 * scoped by wall-clock timing.
 *
 * **3. The record sink is a singleton, for the reason that has no way round it.**
 * `host-record-sink.write(line)` takes a line and nothing else — no context, no key —
 * so two recording clients in one process cannot be told apart by anything the contract
 * carries. Rather than write both recordings into whichever sink was installed last,
 * `installSink` claims a slot and refuses a second, and the README says so as a
 * limitation of the first release. Extending the contract to carry a key here would be a
 * change to `wit/nlipsy.wit`, which is not this package's to make.
 *
 * Nothing in this file is a rule of the protocol. It is an address book.
 */

import { AlreadyInstalledError } from './errors.js';
import type * as wit from './wasm/interfaces/nlipsy-engine-types.js';
import type {
  Config,
  Context,
  HttpOptions,
  Layer,
  Message,
  Rejection,
  RecordSink,
  TokenAuthority,
  Transport,
} from './types.js';

/** Where an exception the contract has no channel for goes. */
export type HostErrorReporter = (error: unknown, where: string) => void;

/**
 * The default reporter: say so on `console.error`.
 *
 * Not silence. The three places that reach here — the configuration functions, the
 * authority's three answers, a sink's failure — are all places where the contract
 * declares no `result`, so the alternative to reporting is a value the deployment never
 * chose being used as though it had.
 */
export const reportToConsole: HostErrorReporter = (error, where) => {
  console.error(`nlipsy: ${where} threw; the default was used instead`, error);
};

/** One deployment's five answers, already resolved against the engine's defaults. */
export interface Deployment {
  /** For logs and for a dispatcher. */
  name: string;
  /** The clause-6 switches. */
  config: Config;
  /** What a clause-5 finding does, per direction. */
  rejection: Rejection;
  /** What the ECMA-431 §6 binding inside the component does. */
  http: HttpOptions;
  /** The user's handler. */
  answer(request: Message, cx: Context): Message | Promise<Message>;
  /** The user's layers, outermost first. */
  layers: Layer[];
  /** The user's authority, with every answer optional. */
  authority: TokenAuthority;
  /** Where an unlowerable exception is reported. */
  onHostError: HostErrorReporter;
}

/**
 * The engine's own three configuration answers, already in the contract's shapes.
 *
 * Kept beside the deployment slot because the four configuration reads under `src/host/`
 * are **synchronous** — `host-endpoint.conformity()` and its two neighbours are `func()`
 * returning a record, with no error channel and no `await` available — so a read that
 * throws has to answer *something* without going back to the component for it. This is
 * that something: what `engine.default-config`, `default-rejection` and
 * `default-http-options` said when the end-point was installed, lowered once at that
 * point so that the fallback is known to be producible rather than hoped to be.
 *
 * Not a cache of the user's values. It is deliberately the *engine's* defaults: a
 * deployment whose own configuration object cannot be read has told the host nothing, and
 * the engine's own answer is the only honest substitute.
 */
export interface EngineDefaults {
  /** `engine.default-config`. */
  config: wit.Config;
  /** `engine.default-rejection`. */
  rejection: wit.Rejection;
  /** `engine.default-http-options`. */
  http: wit.HttpOptions;
}

let defaults: EngineDefaults | undefined;

/** Remember what the engine answered, at install time. */
export function rememberEngineDefaults(values: EngineDefaults): void {
  defaults = values;
}

/**
 * What the engine last answered, or `undefined` before any end-point was installed.
 *
 * `undefined` is not a state a host callback can reach — nothing asks these questions
 * until a deployment is installed, and installing one fills this in first — so a reader
 * that finds it empty has a failure with nothing to fall back to and says so.
 */
export function engineDefaults(): EngineDefaults | undefined {
  return defaults;
}

let installed: Deployment | undefined;

/**
 * Claim the one deployment slot.
 *
 * @returns the function that releases it. Releasing twice is a no-op; releasing a slot
 *          another installation has since claimed does nothing to that one.
 */
export function installDeployment(deployment: Deployment): () => void {
  if (installed !== undefined) {
    throw new AlreadyInstalledError(
      `an end-point (${installed.name})`,
      'The component asks its host for one end-point and hands no key with the ' +
        'question — `host-endpoint.name()`, `.conformity()` and `host-layers.count()` ' +
        'take no arguments — so one process serves one end-point. Close the first with ' +
        '`close()`, or run the second in its own process or worker.',
    );
  }
  installed = deployment;
  return () => {
    if (installed === deployment) installed = undefined;
  };
}

/** The installed deployment, or `undefined` when the component asks and none is. */
export function deployment(): Deployment | undefined {
  return installed;
}

// ---------------------------------------------------------------------------
// The record sink
// ---------------------------------------------------------------------------

interface SinkSlot {
  sink: RecordSink;
  onHostError: HostErrorReporter;
}

let sinkSlot: SinkSlot | undefined;

/** Claim the one record-sink slot. @returns the function that releases it. */
export function installSink(sink: RecordSink, onHostError: HostErrorReporter): () => void {
  if (sinkSlot !== undefined) {
    throw new AlreadyInstalledError(
      'a record sink',
      '`host-record-sink.write(line)` carries no context, so two recordings in one ' +
        'process cannot be told apart. Close the first recording client, or run the ' +
        'second in its own process or worker.',
    );
  }
  const slot: SinkSlot = { sink, onHostError };
  sinkSlot = slot;
  return () => {
    if (sinkSlot === slot) sinkSlot = undefined;
  };
}

/** The installed sink, or `undefined`. */
export function sink(): SinkSlot | undefined {
  return sinkSlot;
}

// ---------------------------------------------------------------------------
// The transport, keyed on the exchange this package minted
// ---------------------------------------------------------------------------

/**
 * Where this package's own exchange numbers start.
 *
 * Above `2**32`, so that a number a host minted is visibly not one the guest minted: the
 * guest's counter starts at 1 and steps by one per exchange. The two could not collide
 * in any case — an exchange the guest minted is an *inbound* one and never reaches
 * `host-transport` — but a number a reader can tell apart in a log line is worth one
 * shift.
 */
const HOST_EXCHANGE_BASE = 1n << 32n;

let nextHostExchange = HOST_EXCHANGE_BASE;

/** A number for one call this package is about to make. */
export function mintExchange(): bigint {
  nextHostExchange += 1n;
  return nextHostExchange;
}

const transports = new Map<bigint, Transport>();

/**
 * Register `transport` for the length of one call.
 *
 * @throws AlreadyInstalledError when `exchange` is already in flight — which can only
 *         happen for a number the *caller* supplied, since a minted one is new.
 * @returns the function that unregisters it, to be called in a `finally`.
 */
export function registerTransport(exchange: bigint, transport: Transport): () => void {
  if (transports.has(exchange)) {
    throw new AlreadyInstalledError(
      `exchange ${exchange}`,
      'Two calls cannot be in flight under one exchange number: the transport for the ' +
        'second could not be told from the first. Leave `context.exchange` at `0n` and ' +
        'one is minted per call.',
    );
  }
  transports.set(exchange, transport);
  return () => {
    transports.delete(exchange);
  };
}

/** The transport for this exchange, or `undefined` when the host installed none. */
export function transportFor(exchange: bigint): Transport | undefined {
  return transports.get(exchange);
}

/** Every slot, emptied. For a test that wants a clean process without one. */
export function resetForTesting(): void {
  installed = undefined;
  sinkSlot = undefined;
  transports.clear();
}
