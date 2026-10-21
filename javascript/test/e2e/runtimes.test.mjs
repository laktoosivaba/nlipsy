/**
 * The two runtimes that are not Node, and what each of them is blocked on.
 *
 * Bun is **not** blocked on the package: `test/node/bun.test.mjs` runs the whole corpus
 * through `dist/` under Bun today, and that suite is not skipped. What Bun cannot do is
 * *serve*, and the reason is the shim's rather than jco's.
 *
 * Chromium is a **client only** and always will be: an incoming `wasi:http/handler` on a
 * socket is not something a page can do. Its route is `createClient({ transport })` over
 * `fetch`, which is `examples/browser-client/` — and which needs the third item of the
 * skip reason, since a `fetch` is an await on the event loop. It is not driven from here
 * because a browser harness is two more dependencies for a suite that cannot pass on
 * either count; the example is the written half, and the README says so.
 */
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';

import { BLOCKERS, RUN, announce } from './reason.mjs';

announce('Bun and the browser', BLOCKERS.handler, BLOCKERS.eventLoop);

const here = path.dirname(fileURLToPath(import.meta.url));

describe.skipIf(!RUN)('other runtimes', () => {
  it('Bun serves an end-point over the shim’s HttpServer', () => {
    const done = spawnSync('bun', ['run', path.join(here, 'scripts/bun-serve.mjs'), '18432'], {
      encoding: 'utf8',
      timeout: 60_000,
    });
    // Today: `DOMException [DataCloneError]: Found invalid value in transferList.`
    expect(`${done.stdout}${done.stderr}`).toContain('status 200');
  });
});
