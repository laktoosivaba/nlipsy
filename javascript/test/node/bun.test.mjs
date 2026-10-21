/**
 * The corpus through the package, under **Bun**.
 *
 * A second runtime is a claim the README makes, so it is a test rather than a sentence.
 * It runs the same `test/corpus-runner.mjs` over the same `dist/` in a Bun process and
 * compares the last line of its report — which is the number, and the number is the gate.
 *
 * Skipped, with the reason printed, where `bun` is not on the PATH: a contributor without
 * it should not be told their change broke something.
 */
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';

const here = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(here, '../bun-corpus.mjs');

const bun = spawnSync('bun', ['--version'], { encoding: 'utf8' });
const HAVE_BUN = bun.status === 0;
if (!HAVE_BUN) {
  console.log('\n[bun] SKIPPED — `bun` is not on the PATH. Install Bun 1.4 or later to run it.\n');
}

describe.skipIf(!HAVE_BUN)('bun', () => {
  it('runs all 125 conformance cases through the package', () => {
    const done = spawnSync('bun', ['run', script], { encoding: 'utf8', timeout: 180_000 });
    if (done.status !== 0) throw new Error(`bun exited ${done.status}:\n${done.stderr}`);
    const line = done.stdout.trim().split('\n').pop();
    console.log(`bun ${bun.stdout.trim()}: ${line}`);
    expect(line).toBe('conformance: 125 passed, 0 skipped, 0 failed, of 125 cases');
  });
});
