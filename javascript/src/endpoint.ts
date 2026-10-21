/**
 * Installing a deployment, and driving the engine's composed order over it.
 *
 * `createEndpoint` claims the one deployment slot `registry.ts` describes and resolves
 * the three configuration blocks against the engine's own defaults, so that the four
 * configuration reads the component performs once per call are property reads rather
 * than round trips. What it hands back is a handle with two things on it: the resolved
 * values, for a deployment that wants to log what it is running under, and `serve`,
 * which is one call into `engine.serve`.
 *
 * **`serve` composes nothing.** Decode, clause 6's inbound step, the layers outermost
 * first, `answer` at the bottom with the finishing step under it, the layers back
 * innermost first, clause 6's idempotent half over whatever came back, and on any
 * failure the engine's own rules instead — every one of those happens inside the
 * component. This function hands it bytes and reads what it produced.
 */

import { engine } from './glue.js';
import {
  fromWitServed,
  toWitConfig,
  toWitContext,
  toWitHttpOptions,
  toWitRejection,
} from './boundary.js';
import { defaultConfig, defaultHttpOptions, defaultRejection } from './defaults.js';
import { installDeployment, rememberEngineDefaults, reportToConsole } from './registry.js';
import type { Context, Endpoint, HttpOptions, Config, Rejection, Served, Wire } from './types.js';

/** The handle `createEndpoint` returns: what it resolved, and what it can be asked. */
export interface InstalledEndpoint {
  /** The name the component reads for its logs. */
  readonly name: string;
  /** The clause-6 switches, the user's over the engine's defaults. */
  readonly config: Config;
  /** What a clause-5 finding does, per direction. */
  readonly rejection: Rejection;
  /** What the ECMA-431 §6 binding inside the component does. */
  readonly http: HttpOptions;
  /**
   * One exchange, over the bytes of one frame.
   *
   * For a deployment whose transport is not the component's own HTTP binding — a
   * WebSocket frame, a queue message, a test. An HTTP deployment calls {@link serve}
   * from `serve.ts` instead and never touches this: there the bytes enter through
   * `wasi:http/handler`, inside the component.
   *
   * It never fails. Every failure is a {@link Served} with an `outcome` and a reply the
   * peer can read, because a binding that has to invent a body for a failure is a
   * binding that will invent a different one from the next binding.
   */
  serve(request: Uint8Array, wire?: Wire, cx?: Context): Promise<Served>;
  /** Release the deployment slot. Idempotent. */
  close(): void;
}

/**
 * Install one deployment and resolve its configuration.
 *
 * @throws AlreadyInstalledError when a deployment is already installed — one process
 *         serves one end-point, for the reason `registry.ts` gives.
 */
export async function createEndpoint(options: Endpoint): Promise<InstalledEndpoint> {
  const [config, rejection, http] = await Promise.all([
    defaultConfig(),
    defaultRejection(),
    defaultHttpOptions(),
  ]);
  // Lowered once, here, and remembered: the four configuration reads the component makes
  // once per call are synchronous and have no error channel, so a read that throws on the
  // deployment's own object has to answer without going back to the component. What it
  // answers is this — the engine's defaults, converted at a point where a failure is a
  // failure of `createEndpoint` rather than of an exchange in flight.
  rememberEngineDefaults({
    config: toWitConfig(config),
    rejection: toWitRejection(rejection),
    http: toWitHttpOptions(http),
  });
  const resolved = {
    name: options.name ?? 'nlipsy',
    config: { ...config, ...options.config },
    rejection: options.rejection ?? rejection,
    http: { ...http, ...options.http },
    answer: options.answer.bind(options),
    layers: options.layers ?? [],
    authority: options.authority ?? {},
    onHostError: options.onHostError ?? reportToConsole,
  };
  const release = installDeployment(resolved);

  return {
    name: resolved.name,
    config: resolved.config,
    rejection: resolved.rejection,
    http: resolved.http,
    async serve(request: Uint8Array, wire: Wire = 'json', cx?: Context): Promise<Served> {
      return fromWitServed(await engine.serve(request, wire, toWitContext(cx)));
    },
    close: release,
  };
}
