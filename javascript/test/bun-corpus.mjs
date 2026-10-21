// The corpus through the package, printed as one line. Run under Bun by
// `test/node/bun.test.mjs`, and runnable by hand: `bun run test/bun-corpus.mjs`.
import * as api from '../dist/index.js';
import { corpus } from './spec/corpus.mjs';
import { format, runCorpus } from './corpus-runner.mjs';

const parsed = corpus();
if (parsed.length !== 125) throw new Error(`expected 125 cases, found ${parsed.length}`);
console.log(format(await runCorpus(api, parsed)).split('\n').pop());
