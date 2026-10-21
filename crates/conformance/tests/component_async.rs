//! The four properties of the built component that the corpus cannot reach, from Rust
//! and with no JavaScript anywhere.
//!
//! **(i) The composed order is the engine's.** The host supplies layers as a
//! `before`/`after` pair per index and the *guest* composes them into
//! `nlipsy-core`'s `Middleware` stack — outermost first on the way in, innermost first on
//! the way out, `Conformity::finish` underneath all of it. That is the contract's design
//! (`wit/README.md`, *The design*), and this is where the order
//! is asserted rather than assumed: a layer's `after` half sees the reply that goes on
//! the wire, §6.2.1's token included.
//!
//! **(ii) A synchronous export can be re-entered during an asynchronous task.** A host
//! callback that asks the engine a question is an ordinary thing to write — the
//! JavaScript wrapper does it, and so does a Rust handler that wants `engine.check` over
//! a submessage it is about to forward. It works, with one condition: the host's `answer`
//! must have handed the task back to the scheduler at least once before it re-enters.
//! The canonical ABI clears the instance's `may_enter` flag for the duration of a lifted
//! call and an asynchronous export only gets it back once it has returned to its event
//! loop; wasmtime calls the host's import while the guest is still on the stack, so a
//! re-entry on the *first* poll of that future is `wasm trap: cannot enter component
//! instance`. One yield is enough, and every real handler has one — it is what `await`
//! on anything at all does. `crates/conformance/src/component.rs` is where the yield is.
//!
//! **(iii) Two exchanges in flight keep their own context.** Two `serve-message` calls
//! whose `answer`s resolve in reverse order, each with its own `context`. The token each
//! reply carries is minted from *its own* context's peer, which is what shows that the
//! guest's per-exchange context does not leak between concurrent tasks — and the context
//! is a record by value now, so there is no longer even a handle that could.
//!
//! **(iv) The client half**, which the corpus cannot reach: `conformance/cases/` is
//! clause 5 and clause 6 and has no case kind for an agent *calling* an end-point. A
//! replaying client and one over the host's transport are the end-to-end proof the other
//! half of the world gets until phase 2 wires `wasi:http`.

#![cfg(feature = "component")]

use std::future::Future;
use std::task::Poll;

use nlipsy_conformance::component::{
    Answer, Before, CaseAuthority, CaseEndpoint, CaseLayer, CaseTransport, Exchange,
    ExchangeDetail, HttpDetail, Instance, ServedOutcome, Turn, Wire, block_on, context_of,
    default_config_of, message_in, message_out, submessage_in,
};
use nlipsy_protocol::{Format, Message, Submessage};

/// Every token of a message, as text, in order.
fn tokens(message: &Message) -> Vec<String> {
    message
        .tokens()
        .filter_map(Submessage::content_as_str)
        .map(str::to_owned)
        .collect()
}

/// Poll two futures to completion together.
///
/// Both are polled on every wake, so neither can be starved by the other — which is the
/// property the assertion needs: if the runtime ran the two exchanges one after the
/// other, the first end-point would wait for a label the second has not yet written and
/// the test would hang rather than pass.
async fn both<A: Future, B: Future>(first: A, second: B) -> (A::Output, B::Output) {
    let mut first = std::pin::pin!(first);
    let mut second = std::pin::pin!(second);
    let mut done_first = None;
    let mut done_second = None;

    std::future::poll_fn(|cx| {
        if done_first.is_none() {
            if let Poll::Ready(value) = first.as_mut().poll(cx) {
                done_first = Some(value);
            }
        }
        if done_second.is_none() {
            if let Poll::Ready(value) = second.as_mut().poll(cx) {
                done_second = Some(value);
            }
        }
        if done_first.is_some() && done_second.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;

    (
        done_first.expect("polled to completion"),
        done_second.expect("polled to completion"),
    )
}

#[test]
fn the_guest_composes_the_hosts_layers_in_the_engines_own_order() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    // §6.2.1 on, so that `finish` has something to do a layer's `after` half can see:
    // minting is in `finish` and nowhere else, and `finish` is at the *bottom* of the
    // stack precisely so that a layer persisting what it minted sees the reply that goes
    // on the wire.
    instance.host().config = default_config_of(true);
    instance.host().authority = CaseAuthority::FromPeer;
    instance.host().endpoint = CaseEndpoint::answering(Answer::Plain(Box::new(message_out(
        &Message::text("answered"),
    ))));
    instance.host().layers = vec![
        CaseLayer {
            label: "outer".to_owned(),
            before: Before::Pass,
            after_suffix: Some("+outer".to_owned()),
        },
        CaseLayer {
            label: "inner".to_owned(),
            before: Before::Pass,
            after_suffix: Some("+inner".to_owned()),
        },
    ];

    let request = message_out(&Message::text("hi"));
    let served = instance
        .enter(|store, engine| {
            let clause6 = engine.nlipsy_engine_engine();
            block_on(store.run_concurrent(async |accessor| {
                clause6
                    .call_serve_message(accessor, request, context_of(Some("peer-1"), 0))
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`engine.serve-message`");

    assert!(matches!(served.outcome, ServedOutcome::Ok));
    let reply = message_in(&served.reply);

    // The way in is outermost first and the way out is innermost first, which is what
    // `Next::run` does and what a host splitting the seam in two must not be able to
    // change.
    let host = instance.host();
    assert_eq!(host.entered, ["before:outer", "before:inner"]);
    assert_eq!(host.resolved, ["after:inner", "after:outer"]);
    assert_eq!(
        reply.content_as_str(),
        Some("answered+inner+outer"),
        "the innermost layer edits the reply first and the outermost last"
    );

    // And `finish` ran *underneath* both of them: §6.2.1's token is on the reply the
    // layers unwound over, so the after-half of a layer that persisted it would have seen
    // the message that goes on the wire.
    assert_eq!(tokens(&reply), ["tok-peer-1"]);
    assert!(!served.withheld);
}

#[test]
fn a_synchronous_export_can_be_re_entered_during_an_asynchronous_task() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    // The request carries a token whose content is not a string, which clause 5 reports
    // as one `warning` and the default inbound policy logs rather than rejects — so
    // `engine.check` has exactly one finding to report and the re-entered call's answer
    // is a number that could not have been guessed.
    let mut asking = Message::text("ask");
    asking.submessages.push(Submessage::new(
        Format::Token,
        "conversation",
        nlipsy_protocol::Content::Number(7.into()),
    ));

    instance.host().endpoint = CaseEndpoint::answering(Answer::ViaCheck);

    let served = instance
        .enter(|store, engine| {
            let clause6 = engine.nlipsy_engine_engine();
            block_on(store.run_concurrent(async |accessor| {
                clause6
                    .call_serve_message(accessor, message_out(&asking), context_of(None, 0))
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`engine.serve-message`");

    // The outcome first: a refused re-entry surfaces as `failed(internal(..))` with the
    // trap in its detail, and asserting the reply alone would report it as "wrong text".
    assert!(
        matches!(served.outcome, ServedOutcome::Ok),
        "the re-entered `check` did not complete: {:?}",
        served.outcome
    );
    assert_eq!(
        message_in(&served.reply).content_as_str(),
        Some("checked 1"),
        "the host's `answer` called `engine.check` from inside itself and got its findings"
    );
}

#[test]
fn two_exchanges_in_flight_keep_their_own_context() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    instance.host().config = default_config_of(true);
    instance.host().authority = CaseAuthority::FromPeer;
    instance.host().endpoint = CaseEndpoint {
        turns: vec![
            Turn {
                when: "ask-a".to_owned(),
                label: "A".to_owned(),
                // A does not answer until B has been entered, so a serialised run hangs
                // instead of passing.
                after: "B".to_owned(),
                answer: Answer::Plain(Box::new(message_out(&Message::text("reply-for-A")))),
            },
            Turn {
                when: "ask-b".to_owned(),
                label: "B".to_owned(),
                after: String::new(),
                answer: Answer::Plain(Box::new(message_out(&Message::text("reply-for-B")))),
            },
        ],
    };

    let ask_a = message_out(&Message::new(Format::Text, "english", "ask-a"));
    let ask_b = message_out(&Message::new(Format::Text, "english", "ask-b"));

    let (served_a, served_b) = instance
        .enter(|store, engine| {
            let clause6 = engine.nlipsy_engine_engine();
            block_on(store.run_concurrent(async |accessor| {
                both(
                    clause6.call_serve_message(accessor, ask_a, context_of(Some("A"), 0)),
                    clause6.call_serve_message(accessor, ask_b, context_of(Some("B"), 0)),
                )
                .await
            }))
        })
        .expect("run_concurrent");

    let served_a = served_a.expect("`serve-message` for A");
    let served_b = served_b.expect("`serve-message` for B");

    let reply_a = message_in(&served_a.reply);
    let reply_b = message_in(&served_b.reply);

    // Unswapped: each call got its own answer.
    assert_eq!(reply_a.content_as_str(), Some("reply-for-A"));
    assert_eq!(reply_b.content_as_str(), Some("reply-for-B"));

    // Each carries a token minted from *its own* context's peer. If the guest parked one
    // context where two tasks could see it, these would be the same string — and it
    // cannot, because the context is a value that travels with the call.
    assert_eq!(tokens(&reply_a), ["tok-A"]);
    assert_eq!(tokens(&reply_b), ["tok-B"]);

    let host = instance.host();
    // Both were entered before either answered, and the *second* answered first.
    assert_eq!(host.entered, ["A", "B"], "both were in flight at once");
    assert_eq!(
        host.resolved,
        ["B", "A"],
        "and they resolved in reverse order"
    );
}

/// A layer that answers from `before` short-circuits, and `close` still runs.
///
/// The corpus asserts the *protocol* half of this in `readings/51`–`/55`; what is here is
/// the composition half, which the corpus cannot see: the handler is never asked and the
/// answering layer's own `after` is never called, while the layer *above* it still
/// unwinds.
#[test]
fn a_layer_that_answers_from_before_skips_the_handler_and_its_own_after() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    instance.host().endpoint = CaseEndpoint::answering(Answer::Plain(Box::new(message_out(
        &Message::text("the handler answered"),
    ))));
    instance.host().layers = vec![
        CaseLayer {
            label: "outer".to_owned(),
            before: Before::Pass,
            after_suffix: Some("+outer".to_owned()),
        },
        CaseLayer {
            label: "cache".to_owned(),
            before: Before::Answer(Box::new(message_out(&Message::text("from the cache")))),
            after_suffix: Some("+cache".to_owned()),
        },
    ];

    let request = message_out(&Message::text("hi"));
    let served = instance
        .enter(|store, engine| {
            let clause6 = engine.nlipsy_engine_engine();
            block_on(store.run_concurrent(async |accessor| {
                clause6
                    .call_serve_message(accessor, request, context_of(None, 0))
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`engine.serve-message`");

    assert!(matches!(served.outcome, ServedOutcome::Ok));
    assert_eq!(
        message_in(&served.reply).content_as_str(),
        Some("from the cache+outer"),
        "the layer above the one that answered still unwinds; the answering layer's own \
         `after` does not run"
    );
    let host = instance.host();
    assert_eq!(host.entered, ["before:outer", "before:cache"]);
    assert_eq!(host.resolved, ["after:outer"]);
    assert!(
        instance.host().endpoint.turns.len() == 1,
        "the handler's turn was never taken"
    );
}

/// The client half, which the corpus cannot reach.
///
/// `conformance/cases/` is clause 5 and clause 6 — what a message *is* and what an
/// end-point *owes* — and says nothing about an agent calling one. `client.replay` needs
/// no transport at all, because a recording is matched against *decoded* requests inside
/// the engine, so it is the cheapest end-to-end proof there is of the other half of the
/// world: encode, carry, decode, and the derived views of `reply` computed in Rust so
/// that a wrapper has no rule to port.
#[test]
fn a_recording_answers_a_request_at_the_cursor_the_host_keeps() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    let first =
        Message::text("What is Ecma?").with_submessage(Submessage::token("conversation", "c-1"));
    let answered = Message::text("A standards body.")
        .with_submessage(Submessage::token("conversation", "c-1"))
        .with_submessage(Submessage::token("authentication_srv-7", "a-7").with_label("seq-1"));
    let second = Message::text("And TC56?");
    let later = Message::text("Standards for agents.");

    // `Recorded`'s spelling on disk, built here with `serde_json` exactly as a fixture
    // file holds it — the request body as it went out, the status, and the reply body.
    let line = |request: &Message, body: &Message| {
        serde_json::json!({
            "request": request.to_json_string(),
            "status": 200,
            "body": body.to_json_string(),
        })
        .to_string()
    };
    let recording = format!("{}\n{}\n", line(&first, &answered), line(&second, &later));

    // The cursor is the host's: `0`, then `1`. The guest holds no state, which is what
    // makes `replay` a free function.
    let reply = instance
        .enter(|store, engine| {
            block_on(engine.nlipsy_engine_client().call_replay(
                &mut *store,
                &recording,
                0,
                &message_out(&first),
                Wire::Json,
            ))
        })
        .expect("`client.replay`")
        .expect("the recording answers this request");

    // The derived views, computed in Rust and carried as fields.
    assert_eq!(reply.text.as_deref(), Some("A standards body."));
    assert_eq!(reply.conversation_tokens, ["c-1"]);
    assert_eq!(reply.authentication_tokens, ["a-7"]);
    assert!(!reply.is_error);
    assert_eq!(reply.bytes as usize, answered.to_json_string().len());

    // And the list the other two are derived from, which is what §6.2 ¶2 obliges this
    // caller to put on its next request: the whole submessages, subformat and label
    // included, and not the keys above.
    let returned: Vec<Submessage> = reply.tokens.iter().map(submessage_in).collect();
    assert_eq!(returned, answered.submessages);

    // The cursor moved on by the host, and the second exchange answers the second
    // request.
    let next = instance
        .enter(|store, engine| {
            block_on(engine.nlipsy_engine_client().call_replay(
                &mut *store,
                &recording,
                1,
                &message_out(&second),
                Wire::Json,
            ))
        })
        .expect("`client.replay`")
        .expect("the recording answers this request too");
    assert_eq!(next.text.as_deref(), Some("Standards for agents."));

    // A request the exchange at the cursor is not for is **refused** rather than answered
    // from the next one: if the client's encoder changed, the next recorded exchange
    // belongs to a request that is no longer being sent.
    let wrong = instance
        .enter(|store, engine| {
            block_on(engine.nlipsy_engine_client().call_replay(
                &mut *store,
                &recording,
                1,
                &message_out(&first),
                Wire::Json,
            ))
        })
        .expect("`client.replay`");
    let why = wrong.expect_err("the recording is for a different request");
    assert!(
        why.contains("different request"),
        "the refusal names what went wrong: {why}"
    );
}

/// `client.call-through` goes out over the host's `host-transport` import.
///
/// The alternative to `wasi:http/client` for a host whose runtime has no outgoing HTTP —
/// the browser, until the preview3 shim's `client` is written — and the path this test
/// drives, because phase 1 has not wired `wasi:http` yet.
#[test]
fn a_client_over_the_hosts_transport_encodes_carries_and_decodes() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    let asked = Message::text("hi");
    let answered = Message::text("hello");

    instance.host().transport = CaseTransport {
        queue: vec![Exchange {
            body: answered.to_json_vec(),
            detail: ExchangeDetail::Http(HttpDetail {
                status: 200,
                retry_after_ms: None,
            }),
        }],
        sent: Vec::new(),
    };

    let reply = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call_through(
                        accessor,
                        message_out(&asked),
                        Wire::Json,
                        context_of(Some("peer-1"), 0),
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call-through`")
        .expect("the transport answered");

    assert_eq!(reply.text.as_deref(), Some("hello"));
    assert!(!reply.is_error);

    // The request went out on the wire the caller named, encoded by the engine — which is
    // what pins a client's encoding rather than leaving it to each binding.
    let sent = &instance.host().transport.sent;
    assert_eq!(sent.len(), 1, "one exchange, one body");
    assert_eq!(
        String::from_utf8_lossy(&sent[0]),
        asked.to_json_string(),
        "`encode_request` is what wrote it"
    );
}
