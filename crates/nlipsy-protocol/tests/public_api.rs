//! The crate as a consumer sees it.
//!
//! The unit tests inside the modules can reach private items and can build the message
//! types by struct literal, neither of which a user of the published crate can do. This
//! file is compiled as a separate crate on purpose: it is what notices that
//! `#[non_exhaustive]` has walled something off, or that an item the README talks about
//! is not actually exported.

use nlipsy_protocol::{Content, Format, Message, ParseError, Severity, Submessage, Wire};

/// TR/113 §11.1's opening exchange, as a peer would build and read it.
#[test]
fn a_message_can_be_built_read_and_answered_from_outside_the_crate() {
    let question = Message::from_json_bytes(
        br#"{"format":"text","subformat":"english","content":"What is Ecma?",
             "submessages":[{"format":"token","subformat":"conversation","content":"c1"}]}"#,
    )
    .expect("TR/113 §11.1");

    assert_eq!(question.format, Format::Text);
    assert_eq!(
        question.extract_text(None).as_deref(),
        Some("What is Ecma?")
    );
    assert_eq!(question.tokens().count(), 1);
    assert!(question.validate().is_empty());

    // Built through the constructors, then adjusted through the public fields — the
    // path `#[non_exhaustive]` leaves open.
    let mut answer = Message::text("An international standards body.");
    answer.control = Some(false);
    answer.label = Some("assistant".into());
    let answer = answer
        .message_type("Response")
        .with_submessage(Submessage::token("conversation", "c1").with_label("session"));

    assert_eq!(
        Message::from_json_str(&answer.to_json_string()).expect("its own output"),
        answer
    );
}

/// ECMA-430 §6.2 and §6.4, built and classified from outside the crate.
#[test]
fn a_token_message_and_a_control_response_are_reachable_through_the_public_api() {
    // §6.2 ¶2 obliges an end-point to echo every token submessage a message contains,
    // and §5 ¶2 calls the leading object a submessage too. `tokens()` cannot yield it,
    // so a consumer needs the predicates to see it at all.
    let credential = Message::from_json_str(
        r#"{"format":"token","subformat":"authentication_9.2.3.5","content":"opaque"}"#,
    )
    .expect("a message may be a token");
    assert!(credential.is_token() && credential.is_authentication_token());
    assert!(!credential.is_conversation_token());
    assert_eq!(credential.tokens().count(), 0);

    // §6.4 ¶3 — "a message with MessageType field set to 'control' containing a
    // submessage with the format field set of 'structured' and subformat of a URI".
    // The constructor builds that shape; the same thing assembled by hand is the
    // reason it exists, and both are public.
    let response = Message::upload_response("https://agent.example.test/nlip/upload/9f2");
    assert!(response.is_control());
    assert_eq!(response.submessages[0].format, Format::Structured);
    assert_eq!(response.submessages[0].subformat, "uri");
    assert!(response.validate().is_empty());
    assert_eq!(
        response.submessages[0],
        Submessage::uri("https://agent.example.test/nlip/upload/9f2")
    );

    // §6.4 ¶2 — the request the answer above belongs to.
    let ask = Message::upload_request("where do I send a large binary upload?");
    assert!(ask.is_control());
    assert_eq!(ask.format, Format::Text);
}

/// Every failure is reachable without matching the enum exhaustively.
#[test]
fn errors_and_findings_read_through_their_accessors() {
    let error = Message::from_json_str(r#"{"format":"text","subformat":"english"}"#)
        .expect_err("content is required");
    assert_eq!(error.path(), "$");
    assert!(error.to_string().contains("content"));
    match &error {
        ParseError::MissingField { field, .. } => assert_eq!(*field, "content"),
        // A `_` arm is what `#[non_exhaustive]` asks of a consumer, and it must be
        // enough: a variant added later must not break this file.
        other => panic!("unexpected error: {other}"),
    }

    let finding = Message::new(Format::Text, "english", 1i64)
        .validate()
        .pop()
        .expect("a number is not text");
    let nlipsy_protocol::Violation { severity, rule, .. } = &finding;
    assert_eq!(*severity, Severity::Error);
    assert_eq!(*rule, "ecma430-5.1.4/text-content");
}

/// ECMA-432 §8 Example 3 — the text fallback — read as JSON and answered as CBOR.
#[test]
fn binary_content_crosses_the_two_wires_unchanged() {
    let example3 = r#"{"MessageType":"Request","Format":"binary","Subformat":"audio/wav;base64",
                       "Content":"UklGRngAAAA=","Submessages":[{"Label":"transcription",
                       "Format":"text","Subformat":"en-US","Content":"Tesla?"}]}"#;

    let from_json = Message::from_json_str(example3).expect("ECMA-432 §8 Example 3");
    assert_eq!(
        from_json.content_as_bytes(),
        Some(&[0x52, 0x49, 0x46, 0x46, 0x78, 0x00, 0x00, 0x00][..])
    );

    // The same document reached through `Content` and the wire it came from.
    let value: serde_json::Value = serde_json::from_str(example3).unwrap();
    assert_eq!(
        Message::from_content(Content::from(value), Wire::Json).unwrap(),
        from_json
    );

    #[cfg(feature = "cbor")]
    {
        let frame = from_json.to_cbor_vec();
        assert!(frame.windows(4).any(|window| window == b"RIFF"));
        assert_eq!(Message::from_cbor_slice(&frame).unwrap(), from_json);
        assert_eq!(
            Message::from_cbor_slice(&frame).unwrap().to_value(),
            from_json.to_value()
        );
    }
}
