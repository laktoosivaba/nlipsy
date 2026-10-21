// The client's routes, its cursor and the shape of a reply, under Node.
import { describe, it } from 'vitest';

import { api } from '../spec/index.mjs';
import { specs } from '../spec/client.mjs';

describe('client', () => {
  for (const spec of specs) {
    it(spec.name, { timeout: 60_000 }, async () => {
      await spec.run(api);
    });
  }
});
