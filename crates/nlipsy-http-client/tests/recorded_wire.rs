//! The client driven by a wire that a live end-point really sent.
//!
//! `fixtures/recorded-wire.jsonl` holds two exchanges recorded from a live NLIP
//! end-point on 2026-09-13. Both the **request** and the reply are stored, and the
//! request is matched on replay, so this file tests the encoder as much as the decoder:
//! if the request ever stops carrying the conversation token, the replay refuses to
//! answer and the test fails rather than passing on a stale reply.
//!
//! The fixture's value is that its bytes were not produced by this crate. A fixture
//! composed from the client's own encoder would agree with a systematic misreading of
//! ECMA-430 §5 just as readily as with a correct one; these bytes come from the other
//! side of the protocol. That is also why it is kept beside the code it tests: with
//! client and server in one workspace, evidence from outside this crate's encoder is
//! what is left (see the README's "What this crate no longer witnesses").
//!
//! The `request` halves were re-spelled once, when `nlipsy-protocol` gained a real
//! `Serialize` impl and began writing a message's fields in §5's order rather than in
//! the alphabetical order a `serde_json::Map` happened to give them. Only the key order
//! moved — same fields, same values — and the `body` halves, which are the evidence,
//! were not touched: they are still the end-point's own bytes.

use nlipsy_core::{Client, Context};
use nlipsy_http_client::{Replay, request};
use nlipsy_protocol::message::Submessage;

/// Recorded from the live end-point. A fixed identifier, so that a replayed request
/// matches the one in the file.
const CONVERSATION: &str = "11111111-1111-4111-8111-111111111111";

/// The correlator travels in a `Context` rather than as an argument to `send`: ECMA-433
/// §6.1.4 makes correlation normative as an AMQP property while ECMA-431 leaves it to a
/// header convention, so it is one concept with a spelling per binding. Nothing about
/// the recorded bytes depends on it — `Replay` never looked at it.
fn correlated(correlator: &str) -> Context {
    Context::new().with_correlator(correlator)
}

fn replay() -> Replay {
    let path = format!(
        "{}/tests/fixtures/recorded-wire.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    Replay::load(&path).expect("the recorded wire should load")
}

#[tokio::test]
async fn the_wire_matches_what_a_live_endpoint_sent() {
    // What this crate puts on the wire today is byte-for-byte what was recorded, so a
    // change to the request's shape cannot pass silently.
    let replay = replay();
    assert_eq!(
        replay.remaining(),
        2,
        "the fixture should hold two exchanges"
    );
    let client = Client::new(replay);

    let first = client
        .send(
            &request(
                "Remember the number 42. Reply with exactly: OK",
                "english",
                CONVERSATION,
            ),
            &correlated("corr-1"),
        )
        .await
        .expect("the recorded request should match");

    assert_eq!(first.text().as_deref(), Some("OK"));
    assert_eq!(first.status(), Some(200));
}

#[tokio::test]
async fn the_endpoint_returns_the_clients_own_conversation_token() {
    // The recorded reply echoes the token that was sent, which is what makes a client
    // free to own its own identifier (§6.2.1) instead of waiting to be handed one. If a
    // future end-point stopped echoing, this fails.
    let client = Client::new(replay());

    let first = client
        .send(
            &request(
                "Remember the number 42. Reply with exactly: OK",
                "english",
                CONVERSATION,
            ),
            &correlated("corr-1"),
        )
        .await
        .unwrap();

    assert_eq!(first.conversation_tokens(), vec![CONVERSATION.to_string()]);
    // And the submessage the end-point really sent, field for field: §6.2 ¶2 obliges
    // this client to return "the exact same format, subformat and content" on its next
    // request, so what the recorded wire pins is the whole submessage and not the key
    // derived from it. The subformat is the one in the file — the end-point echoed
    // `conversation`, unsuffixed — and a client rebuilding the token from the string
    // above would have to guess it.
    assert_eq!(
        first.tokens(),
        vec![Submessage::token("conversation", CONVERSATION)]
    );
}

#[tokio::test]
async fn the_second_turn_answers_a_question_only_the_first_turn_can() {
    // The point of the whole token mechanism, and the reason a conversation is more
    // than a sequence of unrelated requests: "42" appears nowhere in the second
    // request. The end-point could only answer it because the token identified the same
    // conversation, so this test would fail if the client stopped sending it.
    let client = Client::new(replay());

    let _ = client
        .send(
            &request(
                "Remember the number 42. Reply with exactly: OK",
                "english",
                CONVERSATION,
            ),
            &correlated("corr-1"),
        )
        .await
        .unwrap();

    let second = client
        .send(
            &request(
                "What number did I ask you to remember?",
                "english",
                CONVERSATION,
            ),
            &correlated("corr-2"),
        )
        .await
        .unwrap();

    assert_eq!(second.text().as_deref(), Some("42"));
    assert_eq!(second.conversation_tokens(), vec![CONVERSATION.to_string()]);
    assert_eq!(
        second.tokens(),
        vec![Submessage::token("conversation", CONVERSATION)]
    );
}

#[tokio::test]
async fn a_request_the_endpoint_never_saw_is_refused_by_the_recording() {
    // What keeps the fixture honest: the recording is a record of specific requests,
    // not a source of plausible answers.
    let client = Client::new(replay());

    let error = client
        .send(
            &request("a question that was never asked", "english", CONVERSATION),
            &correlated("corr"),
        )
        .await
        .expect_err("the recording should not answer this");

    let message = error.to_string();
    assert!(
        message.contains("Remember the number 42"),
        "the error should name the request the recording does hold: {message}"
    );
}

#[tokio::test]
async fn a_recorded_reply_decodes_into_a_message_with_text() {
    // The recorded body, decoded, is what a caller renders. The join between the wire
    // and a screen is the terminal client's own test (`nlip-client`'s
    // `tests/recorded_wire.rs`, over a copy of this fixture); what belongs here is that
    // the bytes decode into a message at all rather than into `Body::Opaque`.
    let client = Client::new(replay());

    for (question, answer) in [
        ("Remember the number 42. Reply with exactly: OK", "OK"),
        ("What number did I ask you to remember?", "42"),
    ] {
        let reply = client
            .send(
                &request(question, "english", CONVERSATION),
                &correlated("corr"),
            )
            .await
            .unwrap();

        assert!(
            reply.message().is_some(),
            "{question:?} should decode as a message, not as opaque bytes"
        );
        assert!(!reply.is_error());
        assert_eq!(reply.text().as_deref(), Some(answer));
    }
}
