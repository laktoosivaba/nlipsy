// The corpus through the package, under vitest. The runner is `test/corpus-runner.mjs`
// and the expectations are `conformance/cases/`; neither is this file's.
import { describe, it } from 'vitest';

import { api } from '../spec/index.mjs';
import { specs } from '../spec/corpus.mjs';

describe('corpus', () => {
  for (const spec of specs) {
    it(spec.name, { timeout: 120_000 }, async () => {
      await spec.run(api);
    });
  }
});
