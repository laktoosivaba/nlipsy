/**
 * The package's public surface, imported once for every spec.
 *
 * `dist/`, not `src/`: what is tested is what is published — the transpiled glue, the
 * compiled wrapper and the wasm beside them.
 */

import * as api from '../../dist/index.js';

export { api };
