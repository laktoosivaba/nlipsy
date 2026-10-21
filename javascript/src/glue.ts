/**
 * The loader: the transpiled component, and the one import of it in this package.
 *
 * `dist/wasm/` is jco's output over `target/wasm32-wasip3/component/nlipsy_component.wasm`
 * — `scripts/build.mjs` produces it, and the five `--map` arguments there are what bind
 * `src/host/*.ts` to the component's five imports. Importing this module instantiates the
 * component through jco's top-level `await`, which is why this package is ESM only:
 * there is no top-level await in CommonJS.
 *
 * **Every export returns a `Promise`, the ones WIT declares as a plain `func` included.**
 * That is measured against jco 1.39.0 over this artefact and it is *not* what the
 * generated `.d.ts` says — `decode(bytes, wire): Message` there, a `Promise<Message>` at
 * run time (README, "What jco actually renders"). Awaiting a value that is not a promise
 * is harmless, so every call in this package is awaited and the public surface is async
 * throughout. Nothing here papers over the difference with a cast.
 */

import * as component from './wasm/nlipsy.js';

/** ECMA-430 clause 5, as pure functions. */
export const protocol = component.protocol;

/** Clause 6 and the composed order around it. */
export const engine = component.engine;

/** The client half: `wasi:http`, the host's transport, recording and replay. */
export const client = component.client;

/**
 * The incoming half of ECMA-431 §6 — `wasi:http/handler@0.3.0`.
 *
 * Handed whole to `HttpServer` by `serve()`; nothing in this package looks inside it,
 * because the path, the method, the body limit, the status line, `Retry-After` and the
 * correlator are all the component's.
 */
export const handler = component.handler;
