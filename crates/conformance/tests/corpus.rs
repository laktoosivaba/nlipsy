//! The corpus, run against this workspace's implementation.
//!
//! Everything this test knows is where the cases are; the harness is the library beside
//! it, so a second member that wants the same assurance calls `run` rather than copying
//! a runner. `conformance/README.md` is what the cases mean.

use std::path::Path;

use nlipsy_conformance::run;

#[test]
fn the_conformance_corpus_agrees_with_this_implementation() {
    // The corpus sits above every package, in the repository root: one corpus for every
    // implementation is the point of it, and a copy under this crate's `tests/` would be
    // a second corpus that agrees with this workspace by construction.
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/cases");
    let report = run(&cases);
    println!("{report}");

    // Checked before the differences, because a run that found nothing has none: a
    // corpus that moved, or a `.crate` tarball that never carried it, would otherwise
    // read as a pass.
    assert!(
        report.cases > 0,
        "no cases found under {} — the corpus is the test, so an empty one is a failure",
        cases.display()
    );
    assert!(
        report.differences.is_empty(),
        "{} conformance cases failed:\n{report}",
        report.failed
    );
}
