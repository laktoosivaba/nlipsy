/**
 * `nlipsy:engine/host-endpoint` — the deployment's handler and its four configuration
 * values.
 *
 * The target of `--map 'nlipsy:engine/host-endpoint=../host/endpoint.js'`. jco binds an
 * import to a module specifier, so the functions below are fixed and the *deployment*
 * they belong to is looked up in `registry.ts` — see that file for why there is one of
 * those per process and not one per exchange.
 *
 * The four configuration reads happen once per exported call, which is what lets the
 * component build a `Conformity` per call and hold nothing. They must be cheap, and here
 * they are: the values were resolved against the engine's own defaults when the end-point
 * was installed, so each is a property read.
 *
 * A property read can still throw — `rejection` is the deployment's own object rather
 * than a copy of it, so a getter on it runs here — and none of the four has an error
 * channel to lower a throw into. So each runs inside `reported`, which reports through
 * the deployment's `onHostError` and answers the engine's own default: what the
 * component asked for is still a value it can use, and the exchange is still answered,
 * which is what ECMA-430 §6.1 obliges: a throw there would reject the whole exported
 * call, and the exchange would get no reply at all.
 */

import {
  fromWitContext,
  fromWitMessage,
  toWitConfig,
  toWitHttpOptions,
  toWitMessage,
  toWitRejection,
} from '../boundary.js';
import { deployment, engineDefaults } from '../registry.js';
import { NotInstalledError, reported, throwEndpointError } from './lower.js';
import type * as wit from '../wasm/interfaces/nlipsy-engine-types.js';

/** The installed deployment, or an error that says what to do about there being none. */
function installed(what: string) {
  const found = deployment();
  if (found === undefined) throw new NotInstalledError(what);
  return found;
}

/** `Endpoint::name` — for logs and for a dispatcher. Not a protocol value. */
export function name(): string {
  return reported(
    'the end-point’s `name`',
    () => installed('an end-point name').name,
    // The same literal `createEndpoint` uses for a nameless deployment.
    () => 'nlipsy',
  );
}

/** `ConformityConfig` — the optional halves of clause 6 this end-point turns on. */
export function conformity(): wit.Config {
  return reported(
    'the end-point’s `config`',
    () => toWitConfig(installed("an end-point's clause-6 configuration").config),
    () => engineDefaults()?.config,
  );
}

/** `Rejection` — what a clause-5 finding does, inbound and outbound. */
export function rejectionPolicy(): wit.Rejection {
  return reported(
    'the end-point’s `rejection`',
    () => toWitRejection(installed("an end-point's rejection policy").rejection),
    () => engineDefaults()?.rejection,
  );
}

/** `ServerConfig` — what the ECMA-431 §6 binding inside the component does. */
export function httpBinding(): wit.HttpOptions {
  return reported(
    'the end-point’s `http`',
    () => toWitHttpOptions(installed("an end-point's HTTP options").http),
    () => engineDefaults()?.http,
  );
}

/**
 * The user's handler.
 *
 * It returns a **message**: finishing is the engine's and happens at the bottom of the
 * composed layer stack, so there is no shape here for a reply that skipped it.
 *
 * Everything inside the `try` can throw — the user's code, and the conversion of
 * whatever it returned — and all of it becomes an `endpoint-error` rather than escaping
 * the lowering.
 */
export async function answer(request: wit.Message, cx: wit.Context): Promise<wit.Message> {
  try {
    const found = installed('an answer');
    return toWitMessage(await found.answer(fromWitMessage(request), fromWitContext(cx)));
  } catch (error) {
    throwEndpointError(error, 'the end-point’s `answer`');
  }
}
