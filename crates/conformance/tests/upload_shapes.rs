//! The two §6.4 constructors, pinned against the corpus cases that record their output.
//!
//! `ecma-430/44` and `/45` are ordinary message cases: bytes in, decode-and-validate
//! out, runnable by any implementation in any language. What they cannot say on their
//! own is where the bytes came from, because the corpus has no "call this function"
//! shape and should not grow one — a case that named a Rust constructor would be a case
//! only Rust could run.
//!
//! This test is the other end of the pin, and it is here rather than in
//! `nlipsy-protocol` for the reason the corpus is here: it reads the case files. A
//! change to either side is caught — a constructor that stops producing §6.4's shape
//! fails against the recorded bytes, and a case edited to something the constructor
//! does not build fails the same assertion from the other direction.
//!
//! The constructors' *arguments* are read out of the cases rather than written twice:
//! each constructor's only argument is the string it places, so the case already
//! records it — the `ask` is §6.4 ¶2's first-submessage text, and the URI is the content
//! of §6.4 ¶3's `structured`/`uri` submessage. Reading them from there is what keeps the
//! two halves from drifting apart while both still pass.

use std::fs;
use std::path::{Path, PathBuf};

use nlipsy_protocol::Message;
use serde_json::Value;

/// One case file, as a JSON value.
fn case(name: &str) -> Value {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/cases/ecma-430")
        .join(name);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} cannot be read: {why}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|why| panic!("{} is not JSON: {why}", path.display()))
}

/// The one string a case asserts at `path`, or a panic naming what was missing.
fn string_at<'a>(case: &'a Value, path: &[&str]) -> &'a str {
    let mut value = case;
    for step in path {
        value = match step.parse::<usize>() {
            Ok(index) => value
                .get(index)
                .unwrap_or_else(|| panic!("the case has no `{}`", path.join("."))),
            Err(_) => value
                .get(*step)
                .unwrap_or_else(|| panic!("the case has no `{}`", path.join("."))),
        };
    }
    value
        .as_str()
        .unwrap_or_else(|| panic!("`{}` is not a string", path.join(".")))
}

#[test]
fn the_upload_request_constructor_still_produces_the_recorded_shape() {
    let case = case("44-upload-request-constructor.json");
    // §6.4 ¶2's "text format request for the end-point to send large uploads" is the
    // constructor's only argument and the case's own first-submessage content.
    let ask = string_at(&case, &["expect", "message", "content"]);
    let recorded = string_at(&case, &["expect", "canonical_json"]);

    assert_eq!(Message::upload_request(ask).to_json_string(), recorded);
}

#[test]
fn the_upload_response_constructor_still_produces_the_recorded_shape() {
    let case = case("45-upload-response-constructor.json");
    // §6.4 ¶3 puts the address in the submessage, so that is where the argument is.
    let uri = string_at(&case, &["expect", "message", "submessages", "0", "content"]);
    let recorded = string_at(&case, &["expect", "canonical_json"]);

    assert_eq!(Message::upload_response(uri).to_json_string(), recorded);
}

/// And the §6.4 exchange case is those two messages, not a re-spelling of them.
///
/// `ecma-430/38` drives clause 6 over a §6.4 pair, and what it is about is the
/// interaction with §6.3 — the boolean arrives on top of the `MessageType` the
/// constructor wrote. That reading is only worth what the two messages are: a request
/// and a reply composed by hand would make the case evidence about a hand-written
/// document rather than about the shape ¶2 and ¶3 fix.
#[test]
fn the_out_of_band_exchange_carries_exactly_those_two_messages() {
    let exchange = {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/cases/ecma-430/38-out-of-band-exchange.json");
        let text = fs::read_to_string(&path).expect("`ecma-430/38` can be read");
        serde_json::from_str::<Value>(&text).expect("`ecma-430/38` is JSON")
    };

    let ask = string_at(
        &case("44-upload-request-constructor.json"),
        &["expect", "message", "content"],
    )
    .to_owned();
    let uri = string_at(
        &case("45-upload-response-constructor.json"),
        &["expect", "message", "submessages", "0", "content"],
    )
    .to_owned();

    // Compared as the canonical JSON of both, so the assertion is about the messages and
    // not about the key order a case file happens to use.
    let request = Message::from_value(exchange["request"].clone()).expect("the request decodes");
    let reply =
        Message::from_value(exchange["endpoint"]["reply"].clone()).expect("the reply decodes");

    assert_eq!(
        request.to_json_string(),
        Message::upload_request(&ask).to_json_string()
    );
    assert_eq!(
        reply.to_json_string(),
        Message::upload_response(&uri).to_json_string()
    );
}
