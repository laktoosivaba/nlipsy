/**
 * Two projects, and the line between them is what runs today.
 *
 * `node` is the glue's own suites — value conversion, the five host modules, the client's
 * routes and its cursor, and the whole conformance corpus through the package. All of it
 * runs, because every export of the component and every one of its five host imports
 * works under jco today.
 *
 * `e2e` is the three things that do not: an end-point on a socket through
 * `wasi:http/handler`, a client over `wasi:http/client`, and a host callback that
 * suspends across a turn of the event loop — a timer, a socket, a `fetch` — which is the
 * one that matters most, because it is the difference between a handler that awaits a
 * microtask and one that awaits a database. The first two trap inside jco's glue and the
 * third fails differently; `test/e2e/reason.mjs` names all three verbatim. The suites
 * are written, skipped, and print the reason. `NLIPSY_JS_E2E=1` runs them unskipped,
 * which is what `make check-js-e2e` does and what will report the day jco fixes them.
 *
 * Bun is not a project here and Chromium is not either: neither can serve
 * `wasi:http/handler` at all, and what they *can* run
 * — the glue, the corpus, a client over a user's transport — is the same code the `node`
 * project runs. `examples/browser-client/` is the browser's half, written and honest
 * about being untested.
 *
 * Both projects import the built `dist/`, not `src/`, so what is tested is what is
 * published: the transpiled glue, the compiled wrapper and the wasm beside them.
 */

import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: 'node',
          environment: 'node',
          include: ['test/node/**/*.test.mjs'],
          // "The corpus passes" is a claim a reader should see the number behind, and a
          // suite that skips should say why on the way past. vitest buffers a passing
          // file's `console.log` by default, which hides both. A project does not
          // inherit this from the root block, so it is set on each.
          disableConsoleIntercept: true,
          // One component instance per process, one deployment slot in it: two files
          // installing an end-point at the same time would be testing the harness.
          fileParallelism: false,
        },
      },
      {
        test: {
          name: 'e2e',
          environment: 'node',
          include: ['test/e2e/**/*.test.mjs'],
          disableConsoleIntercept: true,
          fileParallelism: false,
        },
      },
    ],
  },
});
