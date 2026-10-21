/**
 * Why the suites in this directory are skipped, in one place.
 *
 * They are written, they are real, and they are what will report on the day jco runs
 * these paths. Printing the reason rather than hiding the tests is the point: a suite
 * that is silently absent reads, six months later, as a suite nobody wrote.
 *
 * **Three seams trap, and only three.** Measured against this artefact with jco 1.39.0
 * and `@bytecodealliance/preview3-shim` 0.11.0 on Node 26.5.0, 2026-10-21 — everything
 * else in this package runs today, all 125 conformance cases included.
 */

/** Set `NLIPSY_JS_E2E=1` — or run `make check-js-e2e` — to run these unskipped. */
export const RUN = process.env.NLIPSY_JS_E2E === '1';

/** The three seams, each with the trap it produces. */
export const BLOCKERS = {
  /** The guest reading the request's path, through the shim's `wasi:http/types`. */
  handler:
    'wasi:http/handler traps: "RuntimeError: wasm trap: out of bounds memory access", and ' +
    'every call into the instance after it is "wasm trap: cannot enter component instance"',
  /** The outgoing half, through the same resources. */
  client: 'wasi:http/client traps: "RuntimeError: memory access out of bounds"',
  /**
   * A host callback suspended across a turn of the event loop. A callback that settles
   * inside the microtask queue is fine; a timer, a socket or a `fetch` is not.
   */
  eventLoop:
    'a host callback that awaits the event loop traps: "RuntimeError: unreachable", then ' +
    '"cannot access a Thread Local Storage value during or after destruction: AccessError" ' +
    'on the calls after it',
};

/**
 * Say, once per suite file, what this suite is waiting for.
 *
 * One blocker per line rather than the whole list four times: a reader wants to know
 * what *this* suite needs, and the README's "What runs today, and what does not" is
 * where the whole story is.
 */
export function announce(what, ...blockers) {
  if (RUN) return;
  const lines = [
    `[${what}] SKIPPED — jco 1.39.0 cannot run this yet:`,
    ...blockers.map((one) => `  · ${one}`),
    '  Written and real; they report the day jco fixes it. See the README,',
    '  "What runs today, and what does not", and test/e2e/reason.mjs for all three.',
    '  Run them with NLIPSY_JS_E2E=1, or `make check-js-e2e`.',
  ];
  console.log(`\n${lines.join('\n')}\n`);
}
