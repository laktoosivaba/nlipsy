// The boundary suite under Node, through vitest. The assertions are in
// `test/spec/boundary.mjs`, which has no test framework in it.
import { describe, it } from 'vitest';

import { api } from '../spec/index.mjs';
import { specs } from '../spec/boundary.mjs';

describe('boundary', () => {
  for (const spec of specs) {
    it(spec.name, { timeout: 60_000 }, async () => {
      await spec.run(api);
    });
  }
});
