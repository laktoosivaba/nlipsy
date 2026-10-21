// The five host modules and the registry, under Node.
import { describe, it } from 'vitest';

import { api } from '../spec/index.mjs';
import { specs } from '../spec/host.mjs';

describe('host', () => {
  for (const spec of specs) {
    it(spec.name, { timeout: 60_000 }, async () => {
      await spec.run(api);
    });
  }
});
