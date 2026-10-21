/**
 * The corpus, as one case for a test harness to name.
 *
 * `conformance/cases/` is the acceptance gate for this package, so this is deliberately
 * one assertion and not 125: what is being asserted is "the corpus passes", and a harness
 * that reported 125 green lines would bury the one number that matters. The differences
 * of every failing case are in the message.
 */

import { cases } from '../generated/corpus.mjs';
import { format, parseCase, runCorpus } from '../corpus-runner.mjs';

/** The corpus, parsed once per run. */
export function corpus() {
  return cases.map(({ id, text }) => ({ id, case: parseCase(text) }));
}

export const specs = [
  {
    name: 'the whole corpus passes',
    async run(api) {
      const parsed = corpus();
      // A corpus that is not there must not read as a clean run: no cases found, so no
      // differences, so nothing to fail on.
      if (parsed.length !== 125) {
        throw new Error(`expected 125 cases, found ${parsed.length}`);
      }
      const report = await runCorpus(api, parsed);
      if (report.failed.length > 0 || report.passed !== 125 || report.skipped !== 0) {
        throw new Error(format(report));
      }
      // Printed rather than only returned: the counts are the gate, and a run that
      // reported one green line would leave a reader to take "the corpus passes" on
      // trust. `crates/conformance` prints the same summary for the same reason.
      console.log(format(report));
      return format(report);
    },
  },
];
