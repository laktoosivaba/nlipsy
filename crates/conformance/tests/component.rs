//! The corpus, run through the built wasm component in wasmtime.
//!
//! The same cases, the same expectations and the same report as `tests/corpus.rs`; what
//! differs is where the answers come from. `crates/conformance/src/component.rs` is the
//! host, and this is what it proves: the artefact answers the whole corpus with no
//! JavaScript anywhere in the picture.
//!
//! It runs only with `--features component`, and `make check-component` is the target
//! that builds the artefact first.

#![cfg(feature = "component")]

use std::path::Path;

use nlipsy_conformance::component::{Corpus, Instance, artefact_path, block_on, context_of};
use nlipsy_conformance::run_with;

#[test]
fn the_conformance_corpus_agrees_with_the_built_component() {
    let mut corpus = Corpus::load().unwrap_or_else(|why| panic!("{why}"));

    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/cases");
    let report = run_with(&cases, &mut corpus);
    println!("component: {}", artefact_path().display());
    println!("{report}");

    assert!(
        report.cases > 0,
        "no cases found under {} — the corpus is the test, so an empty one is a failure",
        cases.display()
    );
    assert_eq!(
        report.skipped, 0,
        "the component is built with the CBOR codec, so no ECMA-432 §7.1 case may be skipped"
    );
    assert!(
        report.differences.is_empty(),
        "{} conformance cases failed through the component:\n{report}",
        report.failed
    );
}

/// The free functions of `engine` that no case reaches.
///
/// The corpus is clause 5 and clause 6; the values a binding needs are the other half of
/// the world and have no case kind, because a status code is not a message. Every one of
/// them is asserted against the crate it was lifted from — `wit/README.md` §3's `engine`
/// block, row for row — so that the component's copy of a value and the binding's cannot
/// drift.
#[test]
fn the_values_the_corpus_cannot_reach_are_the_bindings_own() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    let Instance { store, engine, .. } = &mut instance;
    let clause6 = engine.nlipsy_engine_engine();

    // Clause 6's defaults — the numbers a runner would otherwise re-type, which is
    // why the contract exports them.
    let config = block_on(clause6.call_default_config(&mut *store)).expect("`default-config`");
    let theirs = nlipsy_core::ConformityConfig::default();
    assert_eq!(
        config.issue_conversation_token,
        theirs.issue_conversation_token
    );
    assert_eq!(
        config.issue_authentication_token,
        theirs.issue_authentication_token
    );
    assert_eq!(
        config.max_tokens_per_message as usize,
        theirs.max_tokens_per_message
    );

    let rejection =
        block_on(clause6.call_default_rejection(&mut *store)).expect("`default-rejection`");
    let theirs = nlipsy_core::Rejection::default();
    assert_eq!(
        rejection.inbound.on_error == nlipsy_conformance::component::Disposition::Reject,
        theirs.inbound.on_error == nlipsy_core::Disposition::Reject
    );
    assert_eq!(
        rejection.outbound.on_warning == nlipsy_conformance::component::Disposition::Log,
        theirs.outbound.on_warning == nlipsy_core::Disposition::Log
    );

    // ECMA-431 §6 as the binding's own options — `crates/nlipsy-http` for the three
    // values that have a seed, and `cors: none` for the one that does not, which is what
    // the axum binding does too.
    let options =
        block_on(clause6.call_default_http_options(&mut *store)).expect("`default-http-options`");
    assert_eq!(
        options.max_request_body_size as usize,
        nlipsy_http::DEFAULT_MAX_REQUEST_BODY_SIZE
    );
    assert_eq!(
        options.correlator_header.as_deref(),
        Some(nlipsy_http::CORRELATOR_HEADER)
    );
    assert!(
        options.cors.is_none(),
        "the reference binding serves no browser and has no CORS"
    );

    let statuses = options.status;
    let theirs = nlipsy_http::StatusMap::default();
    assert_eq!(statuses.malformed, theirs.malformed.as_u16());
    assert_eq!(statuses.refused, theirs.refused.as_u16());
    assert_eq!(statuses.too_large, theirs.too_large.as_u16());
    assert_eq!(statuses.unsupported, theirs.unsupported.as_u16());
    assert_eq!(statuses.unavailable, theirs.unavailable.as_u16());
    assert_eq!(statuses.timed_out, theirs.timed_out.as_u16());
    assert_eq!(statuses.internal, theirs.internal.as_u16());
    assert_eq!(statuses.withheld, theirs.withheld.as_u16());
    assert_eq!(statuses.other, theirs.other.as_u16());
}

/// The three rules over an `outcome`, asked of the component rather than restated.
///
/// `owed-by`, `public-message` and `retry-after-of` are what a host building a refusal of
/// its own needs, and the guest's own `wasi:http/handler` asks two of them. Each is
/// compared against `nlipsy-core`'s answer for the same variant, which is the whole
/// point of their being exported: a host that re-derived one would hold a second copy
/// of a rule the engine already states.
#[test]
fn the_rules_over_an_outcome_are_the_engines() {
    use nlipsy_conformance::component::{
        EndpointError, Owed, ServedOutcome as Outcome, Unavailable,
    };

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    let Instance { store, engine, .. } = &mut instance;
    let clause6 = engine.nlipsy_engine_engine();

    // `ok` owes `clause6` like most failures do, and that is not an accident of the
    // match: the success path runs `close` for the same reason a refusal does.
    assert_eq!(
        block_on(clause6.call_owed_by(&mut *store, &Outcome::Ok)).expect("`owed-by`"),
        Owed::Clause6
    );
    assert!(
        block_on(clause6.call_public_message(&mut *store, &Outcome::Ok))
            .expect("`public-message`")
            .is_none(),
        "`ok` is the one outcome with nothing to explain: the reply *is* the answer"
    );

    // The delay a failing outcome asked for, in milliseconds. A binding that matched on
    // `unavailable` itself would hold a second copy of a rule that moves.
    let waiting = Outcome::Failed(EndpointError::Unavailable(Unavailable {
        retry_after_ms: Some(1_500),
        detail: "the index is rebuilding".to_owned(),
    }));
    assert_eq!(
        block_on(clause6.call_retry_after_of(&mut *store, &waiting)).expect("`retry-after-of`"),
        Some(1_500)
    );
    assert_eq!(
        block_on(clause6.call_retry_after_of(&mut *store, &Outcome::Ok)).expect("`retry-after-of`"),
        None,
        "only a failure that named a delay carries one"
    );

    // ECMA-434 §7.3.3: the detail of an internal fault is for the log and never for the
    // peer, and `public-message` is the export that keeps it off the wire.
    let fault = Outcome::Failed(EndpointError::Internal("connection string: …".to_owned()));
    let public = block_on(clause6.call_public_message(&mut *store, &fault))
        .expect("`public-message`")
        .expect("a failure has something to explain");
    assert!(
        !public.contains("connection string"),
        "the detail of an `internal` fault must not reach the peer: {public}"
    );
}

/// `engine.check` and `engine.finish` are two steps of **one** exchange when the caller
/// names one.
///
/// The property `types.context` states and the corpus depends on: `cx.exchange` of `0`
/// means "no exchange named" and the guest mints a fresh number, while any other value
/// travels unchanged to every host callback. A host keying a session on that number
/// would otherwise be handed two exchanges for one.
#[test]
fn the_exchange_number_is_the_callers_when_the_caller_names_one() {
    use nlipsy_conformance::component::{CaseEndpoint, message_out};
    use nlipsy_protocol::Message;

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    instance.host().endpoint = CaseEndpoint::default();

    // An authority that reports the exchange it was asked under, by minting `tok-<n>`.
    // §6.2.1 has to be on for `finish` to reach `mint` at all.
    instance.host().config = nlipsy_conformance::component::default_config_of(true);
    instance.host().authority = nlipsy_conformance::component::CaseAuthority::FromPeer;

    let request = message_out(&Message::text("hi"));
    let reply = message_out(&Message::text("hello"));

    // The authority mints from the *peer*, so the two calls are told apart by the
    // context they carried rather than by a counter of the host's.
    let finished = {
        let Instance { store, engine, .. } = &mut instance;
        block_on(engine.nlipsy_engine_engine().call_finish(
            &mut *store,
            &request,
            &reply,
            &context_of(Some("named"), 42),
        ))
        .expect("`engine.finish`")
    };
    let tokens: Vec<String> = finished
        .message
        .submessages
        .iter()
        .filter_map(|one| match &one.content {
            nlipsy_conformance::component::WitContent::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tokens,
        ["tok-named"],
        "the authority was asked with the context the caller passed"
    );
}
