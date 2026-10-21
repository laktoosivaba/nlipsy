//! ECMA-431 §6, through the component's own `wasi:http/handler@0.3.0`.
//!
//! The binding is the **guest's** (`wit/README.md`, *The design*), so this is where it is
//! checked: an `http::Request` goes in through
//! `wasmtime_wasi_http::p3::bindings::Service::handle`, which finds the export on the
//! instance and needs no binding of the host's, and a status line, a header map and a
//! body come back — which is exactly what a peer sees.
//!
//! What is asserted is `crates/nlipsy-server`'s behaviour, because the two bindings are
//! twins over one crate: the path and the method ECMA-431 §6 fixes, the body limit
//! enforced *while the body is read*, the status from `StatusMap::of`, `Retry-After` from
//! `retry_after_value`, `Content-Type` from `CONTENT_TYPE`, the correlator echoed or
//! minted under `MAX_CORRELATOR_LENGTH`, `405` with `Allow: POST`, `404` with an NLIP
//! body, and the cross-origin headers only for an `Origin` that matches, and nothing
//! else.
//!
//! It runs only with `--features component`, and `make check-component` is the target
//! that builds the artefact first.

#![cfg(feature = "component")]

use nlipsy_conformance::component::{
    Answer, CaseAuthority, CaseEndpoint, Cors, HttpAnswer, Instance, default_http_options_of,
    message_out,
};
use nlipsy_protocol::{Message, Submessage};

/// The end-point every test here is answered by, unless it says otherwise.
fn echoing(instance: &mut Instance, reply: Message) {
    instance.host().endpoint =
        CaseEndpoint::answering(Answer::Plain(Box::new(message_out(&reply))));
    instance.host().authority = CaseAuthority::default();
}

/// The body, decoded as the NLIP message every reply this binding writes is.
fn body_of(answer: &HttpAnswer) -> Message {
    Message::from_json_bytes(&answer.body).unwrap_or_else(|why| {
        panic!(
            "every reply this binding writes is an NLIP message; this one is not ({why}): {}",
            answer.text()
        )
    })
}

#[test]
fn a_clean_post_is_answered_two_hundred_with_the_content_type_and_the_correlator_echoed() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    let asked = Message::text("What is Ecma?")
        .with_submessage(Submessage::token("conversation", "c-1"))
        .to_json_vec();
    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[
                ("content-type", "application/json"),
                ("x-nlip-correlator", "corr-1"),
            ],
            asked,
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.header("content-type"),
        Some(nlipsy_http::CONTENT_TYPE)
    );
    assert_eq!(
        answer.header(nlipsy_http::CORRELATOR_HEADER),
        Some("corr-1"),
        "a sane correlator comes back unchanged, so a chain of end-points can be followed"
    );
    assert!(
        answer.header("retry-after").is_none(),
        "nothing asked the peer to wait"
    );
    assert_eq!(
        answer.header("content-length"),
        Some(answer.body.len().to_string().as_str()),
        "the reply is framed rather than chunked, which is what lets a peer apply its \
         own bound before reading a byte — the argument this binding's own client half \
         makes for the request, and what the axum binding's `json_response` does"
    );

    // And clause 6 ran inside: §6.2's echo is on the reply the handler never touched.
    let reply = body_of(&answer);
    assert_eq!(reply.content_as_str(), Some("hello"));
    let echoed: Vec<&str> = reply
        .tokens()
        .filter_map(Submessage::content_as_str)
        .collect();
    assert_eq!(echoed, ["c-1"], "§6.2 ¶2's echo, through the binding");
}

#[test]
fn a_post_without_a_correlator_is_given_one() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 200);
    let minted = answer
        .header(nlipsy_http::CORRELATOR_HEADER)
        .expect("an exchange that cannot be named in a log line is one nobody can investigate");
    // A UUID version 4, in the spelling `Uuid::new_v4().to_string()` produces — the
    // bytes are `wasi:random/random@0.3.0`'s.
    assert_eq!(minted.len(), 36, "{minted}");
    assert_eq!(minted.as_bytes()[14], b'4', "the version nibble: {minted}");
    assert!(
        matches!(minted.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
        "the variant bits: {minted}"
    );

    // And a second exchange gets a different one, which is the whole point of minting.
    echoing(&mut instance, Message::text("hello"));
    let again = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");
    assert_ne!(again.header(nlipsy_http::CORRELATOR_HEADER), Some(minted));
}

#[test]
fn an_over_long_correlator_is_replaced_visibly() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    // 129 bytes: one past `MAX_CORRELATOR_LENGTH`, which is `nlipsy-server`'s number and
    // this binding's copy of it.
    let long = "x".repeat(129);
    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[(nlipsy_http::CORRELATOR_HEADER, long.as_str())],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    let back = answer
        .header(nlipsy_http::CORRELATOR_HEADER)
        .expect("one is minted when the peer's is refused");
    assert_ne!(back, long, "the substitution is visible to the peer");
    assert_eq!(back.len(), 36);
}

#[test]
fn another_method_on_the_path_is_four_oh_five_with_allow_post_and_the_engines_body() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    let answer = instance
        .http("GET", "http://example.com/nlip", &[], Vec::new())
        .expect("the guest answered");

    assert_eq!(answer.status, 405);
    // The header is the method ECMA-431 §6 names, written by the guest itself: it has no
    // export and no seed, and inventing a constant for it would be an export with
    // neither.
    assert_eq!(answer.header("allow"), Some("POST"));
    assert_eq!(
        answer.header("content-type"),
        Some(nlipsy_http::CONTENT_TYPE)
    );
    // A status line alone tells a peer reading the payload nothing.
    assert_eq!(body_of(&answer), nlipsy_http::method_not_allowed());
}

#[test]
fn a_body_over_the_limit_is_four_thirteen_with_the_limit_named_and_nothing_echoed() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    // A limit the test can exceed without holding a megabyte, set the way a deployment
    // sets one: through `host-endpoint.http-binding`.
    let mut options = default_http_options_of();
    options.max_request_body_size = 64;
    instance.host().http_options = options;

    let asked = Message::text("x".repeat(512))
        .with_submessage(Submessage::token("conversation", "c-1"))
        .to_json_vec();
    assert!(asked.len() > 64);

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[(nlipsy_http::CORRELATOR_HEADER, "corr-1")],
            asked,
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 413);
    assert_eq!(body_of(&answer), nlipsy_http::too_large(64));
    assert!(
        answer.text().contains("64"),
        "the number is what lets a peer split the payload instead of guessing"
    );
    // §6.2 cannot be honoured: the body was never decoded, so no token was ever read.
    // The correlator *is* echoed — it is a header and not a token, and `nlipsy-server`
    // echoes it on the same path.
    assert_eq!(body_of(&answer).submessages.len(), 0);
    assert_eq!(
        answer.header(nlipsy_http::CORRELATOR_HEADER),
        Some("corr-1")
    );
}

#[test]
fn a_path_this_end_point_does_not_serve_is_four_oh_four_in_the_protocol() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    let answer = instance
        .http("POST", "http://example.com/nope", &[], Vec::new())
        .expect("the guest answered");

    assert_eq!(answer.status, 404);
    assert_eq!(
        answer.header("content-type"),
        Some(nlipsy_http::CONTENT_TYPE)
    );
    let body = body_of(&answer);
    assert_eq!(body.message_type.as_deref(), Some("Error"));
    assert!(
        body.content_as_str()
            .is_some_and(|text| text.contains(nlipsy_http::NLIP_PATH)),
        "the hint is the message text rather than a second body shape: {}",
        answer.text()
    );
}

#[test]
fn a_query_string_does_not_move_the_end_point() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip?trace=1",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 200);
    assert_eq!(body_of(&answer).content_as_str(), Some("hello"));
}

#[test]
fn the_trailing_slash_spelling_reaches_the_same_end_point() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    // `/nlip/` is the spelling the ECMA-430 reference server mounts, and the axum
    // binding mounts both on purpose (`crates/nlipsy-server/src/server.rs:402–406`).
    // A peer that moves between the two bindings must not find one of them stricter.
    for target in [
        "http://example.com/nlip/",
        "http://example.com/nlip/?trace=1",
    ] {
        echoing(&mut instance, Message::text("hello"));
        let answer = instance
            .http(
                "POST",
                target,
                &[("content-type", "application/json")],
                Message::text("hi").to_json_vec(),
            )
            .expect("the guest answered");

        assert_eq!(answer.status, 200, "{target}");
        assert_eq!(body_of(&answer).content_as_str(), Some("hello"), "{target}");
    }

    // And no further. The upload leg of ECMA-431 §6.1 is not in this binding, so its
    // path must not be claimed by accident.
    let upload = instance
        .http(
            "POST",
            "http://example.com/nlip/upload",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");
    assert_eq!(upload.status, 404);
}

#[test]
fn a_body_that_ends_early_is_five_hundred_and_not_an_answered_exchange() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    // A prefix that is a complete, valid NLIP message on its own — which is the serious
    // half of the failure: before the fix this was answered `200` with a full clause-6
    // reply, so an end-point that commits a session in its handler had recorded a turn
    // from a message the peer never finished sending.
    let prefix = Message::text("hello world this is a long message").to_json_vec();
    let answer = instance
        .http_broken_body(
            "POST",
            "http://example.com/nlip",
            &[
                ("content-type", "application/json"),
                (nlipsy_http::CORRELATOR_HEADER, "corr-broken"),
            ],
            prefix,
        )
        .expect("the guest answered");

    // The axum binding's answer, code for code and word for word
    // (`Server::unreadable_body`, `crates/nlipsy-server/src/server.rs:528–546`).
    assert_eq!(
        answer.status,
        nlipsy_http::StatusMap::default().internal.as_u16()
    );
    let body = body_of(&answer);
    assert_eq!(body.message_type.as_deref(), Some("Error"));
    assert_eq!(
        body.content_as_str(),
        Some("the request body could not be read")
    );
    // Nothing of clause 6 ran, because there was no request: no echo, and no mint.
    assert_eq!(body.submessages.len(), 0);
    assert!(
        answer.header("retry-after").is_none(),
        "nothing about a broken connection says when to come back"
    );
    // The correlator is a header and not a token, so it is still echoed — the exchange
    // has to be nameable in a log line even when it failed.
    assert_eq!(
        answer.header(nlipsy_http::CORRELATOR_HEADER),
        Some("corr-broken")
    );
}

#[test]
fn a_body_that_ends_cleanly_is_still_two_hundred() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    // The other side of the same future: `stream<u8>` answers `Dropped` for a clean end
    // and for a broken one alike, so the trailers future is what tells them apart — and
    // a body that simply ended must still be an ordinary exchange.
    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[("content-type", "application/json")],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 200);
    assert_eq!(body_of(&answer).content_as_str(), Some("hello"));
}

#[test]
fn a_body_far_over_the_limit_is_four_thirteen_and_the_bound_is_reserve_exact_s() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    let mut options = default_http_options_of();
    options.max_request_body_size = 64;
    instance.host().http_options = options;

    // Four times the limit, where `Vec::reserve`'s amortised growth would have doubled
    // the buffer past the bound before the check fired. What this test pins is the
    // *status*: the allocation bound is `reserve_exact`'s, and it is pinned by the
    // comment beside it in `crates/nlipsy-component/src/http.rs` — the two spellings
    // answer a peer identically and only the bytes held differ
    // (measured at 2.00x and 1.00x).
    let asked = Message::text("x".repeat(64 * 4)).to_json_vec();
    let answer = instance
        .http("POST", "http://example.com/nlip", &[], asked)
        .expect("the guest answered");

    assert_eq!(answer.status, 413);
    assert_eq!(body_of(&answer), nlipsy_http::too_large(64));
}

#[test]
fn a_reply_that_breaks_clause_five_is_the_withheld_code_and_not_internals() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    // An end-point whose own answer breaks §5.2.3: a subformat that is required and
    // empty. The default outbound policy rejects a clause-5 `error`, so `finish`
    // replaces the reply and `served.withheld` is `true`.
    let mut broken = Message::text("hello");
    broken.subformat = String::new();
    instance.host().endpoint =
        CaseEndpoint::answering(Answer::Plain(Box::new(message_out(&broken))));
    instance.host().authority = CaseAuthority::default();

    // `withheld` and `internal` are the same number by default, so one of them has to
    // move for the test to say which field was read. That is the whole reason
    // `status-map` carries `withheld` as a field of its own: a deployment relaying from
    // an end-point it does not control tells the two apart in its logs.
    let mut options = default_http_options_of();
    options.status.withheld = 507;
    instance.host().http_options = options;

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 507);
    // §6.1 still: the replacement is a message, not an empty body.
    assert_eq!(body_of(&answer).message_type.as_deref(), Some("Error"));
}

#[test]
fn an_end_point_that_will_never_answer_is_the_unsupported_code() {
    use nlipsy_conformance::component::EndpointError;

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    instance.host().endpoint = CaseEndpoint::answering(Answer::Failing(Box::new(
        EndpointError::Unsupported("this end-point answers text and nothing else".to_owned()),
    )));

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    // `501`, which is the map's own value for the one error that says retrying changes
    // nothing — and it is not `unavailable`'s, so no `Retry-After` goes out.
    assert_eq!(
        answer.status,
        nlipsy_http::StatusMap::default().unsupported.as_u16()
    );
    assert!(answer.header("retry-after").is_none());
    assert_eq!(body_of(&answer).message_type.as_deref(), Some("Error"));
}

#[test]
fn a_binding_that_names_no_correlator_header_reads_none_and_echoes_none() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    // `correlator-header: none` is a deployment carrying no correlator at all. The
    // contract says the peer's value is not read and none is echoed, while
    // `context.correlator` is still filled — the concept lives in the context and this
    // is only its HTTP spelling.
    let mut options = default_http_options_of();
    options.correlator_header = None;
    instance.host().http_options = options;

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[(nlipsy_http::CORRELATOR_HEADER, "corr-1")],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 200);
    assert_eq!(body_of(&answer).content_as_str(), Some("hello"));
    assert!(
        answer.header(nlipsy_http::CORRELATOR_HEADER).is_none(),
        "a deployment that names no header gets no header, not an empty one"
    );
}

#[test]
fn a_refused_request_carries_the_status_map_s_code_and_retry_after() {
    use nlipsy_conformance::component::{EndpointError, Unavailable};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    instance.host().endpoint = CaseEndpoint::answering(Answer::Failing(Box::new(
        EndpointError::Unavailable(Unavailable {
            retry_after_ms: Some(1_500),
            detail: "the index is rebuilding".to_owned(),
        }),
    )));

    let answer = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");

    // `StatusMap::of` over `Outcome::Failed(Unavailable)`, and the map is the host's.
    assert_eq!(
        answer.status,
        nlipsy_http::StatusMap::default().unavailable.as_u16()
    );
    // Whole seconds, floored at one and never zero: 1 500 ms is `1`.
    assert_eq!(answer.header("retry-after"), Some("1"));
    // §6.1 still: a failure is an NLIP message, and the detail of the failure is the
    // peer's to read here because `unavailable` is safe to show (ECMA-434 §7.3.3 is
    // about `internal`).
    let body = body_of(&answer);
    assert_eq!(body.message_type.as_deref(), Some("Error"));
    assert!(body.content_as_str().is_some());
}

#[test]
fn the_cross_origin_headers_are_emitted_only_for_an_origin_that_matches() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    echoing(&mut instance, Message::text("hello"));

    let mut options = default_http_options_of();
    options.cors = Some(Cors {
        origins: vec!["https://agent.example".to_owned()],
        credentials: true,
    });
    instance.host().http_options = options.clone();

    // A preflight from the origin the deployment named.
    let preflight = instance
        .http(
            "OPTIONS",
            "http://example.com/nlip",
            &[("origin", "https://agent.example")],
            Vec::new(),
        )
        .expect("the guest answered");
    assert_eq!(preflight.status, 204);
    assert!(
        preflight.header("content-length").is_none(),
        "a preflight has no body, and `content-length` beside no body is a header RFC \
         9110 §8.6 does not want"
    );
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some("https://agent.example"),
        "the origin itself and never `*`, which is the only form a credentialed request \
         accepts"
    );
    assert_eq!(
        preflight.header("access-control-allow-credentials"),
        Some("true")
    );
    assert!(
        preflight
            .header("access-control-allow-headers")
            .is_some_and(|value| value.contains(nlipsy_http::CORRELATOR_HEADER)),
        "a custom request header is what triggers the preflight in the first place"
    );

    // The same preflight with no `Origin` at all: **no cross-origin header whatsoever**,
    // never `*`. With no origin there is no CORS answer, so the
    // request is just another method on the path.
    instance.host().http_options = options.clone();
    let bare = instance
        .http("OPTIONS", "http://example.com/nlip", &[], Vec::new())
        .expect("the guest answered");
    assert_eq!(bare.status, 405);
    assert!(bare.header("access-control-allow-origin").is_none());
    assert!(bare.header("access-control-allow-credentials").is_none());

    // And an origin the deployment did not name gets the same nothing.
    instance.host().http_options = options;
    echoing(&mut instance, Message::text("hello"));
    let stranger = instance
        .http(
            "POST",
            "http://example.com/nlip",
            &[("origin", "https://elsewhere.example")],
            Message::text("hi").to_json_vec(),
        )
        .expect("the guest answered");
    assert_eq!(stranger.status, 200, "the exchange is answered either way");
    assert!(stranger.header("access-control-allow-origin").is_none());
}

#[test]
fn a_binding_with_no_cors_answers_options_as_another_method() {
    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    // The default is no CORS, which is what `nlipsy-server` does: it serves no browser.
    let answer = instance
        .http(
            "OPTIONS",
            "http://example.com/nlip",
            &[("origin", "https://agent.example")],
            Vec::new(),
        )
        .expect("the guest answered");

    assert_eq!(answer.status, 405);
    assert_eq!(answer.header("allow"), Some("POST"));
    assert!(answer.header("access-control-allow-origin").is_none());
}

// ---------------------------------------------------------------------------
// The outgoing half: `client.call` over `wasi:http/client@0.3.0`
// ---------------------------------------------------------------------------

/// What the one-shot server saw, and what it answered.
struct Peer {
    /// `127.0.0.1:<port>`.
    address: String,
    /// The request, headers and body, as the socket received it.
    seen: std::sync::mpsc::Receiver<String>,
}

/// A one-request HTTP/1.1 end-point on a loopback port.
///
/// Not a framework: `wasi:http/client`'s outgoing side is what is under test, so what it
/// talks to has to be a socket and nothing more. It reads the whole request — headers,
/// then `Content-Length` bytes — before answering, because a server that replied early
/// would leave the client writing into a closed pipe and the test would be about that.
fn one_shot(status: u16, reply: &Message) -> Peer {
    let body = reply.to_json_vec();
    let mut answer = head(status, body.len(), &[]).into_bytes();
    answer.extend_from_slice(&body);
    answering(answer)
}

/// The same end-point, with extra response headers — `Retry-After`, say.
fn one_shot_with(status: u16, reply: &Message, extra: &[(&str, &str)]) -> Peer {
    let body = reply.to_json_vec();
    let mut answer = head(status, body.len(), extra).into_bytes();
    answer.extend_from_slice(&body);
    answering(answer)
}

/// An end-point whose reply stops halfway: the status line and the headers arrive, the
/// declared `Content-Length` does not, and the socket closes.
///
/// It is the outgoing twin of the incoming case — a peer that stopped sending — and the
/// one a client must report as `interrupted` rather than as an answered exchange whose
/// body happens not to be NLIP.
fn one_shot_truncated(reply: &Message) -> Peer {
    let body = reply.to_json_vec();
    let mut answer = head(200, body.len(), &[]).into_bytes();
    answer.extend_from_slice(&body[..body.len() / 2]);
    answering(answer)
}

/// The response head this harness writes.
fn head(status: u16, length: usize, extra: &[(&str, &str)]) -> String {
    let mut head = format!(
        "HTTP/1.1 {status} \r\nContent-Type: {}\r\nContent-Length: {length}\r\nConnection: close\r\n",
        nlipsy_http::CONTENT_TYPE,
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}

/// Read one whole request off a loopback socket, then write `answer` and close.
fn answering(answer: Vec<u8>) -> Peer {
    use std::io::{Read as _, Write as _};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = format!("127.0.0.1:{}", listener.local_addr().expect("bound").port());
    let (sender, seen) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("the client connected");
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        // Headers first, to the blank line.
        while !request.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => request.push(byte[0]),
            }
        }
        let head = String::from_utf8_lossy(&request).to_lowercase();
        let length = head
            .split("\r\n")
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok());
        match length {
            Some(length) => {
                let mut rest = vec![0u8; length];
                let _ = socket.read_exact(&mut rest);
                request.extend_from_slice(&rest);
            }
            // No `Content-Length` means the body is chunked, and it still has to be read
            // before the reply goes out: a server that answered early would leave the
            // client writing into a closed socket, and the exchange would fail for a
            // reason that is this harness's rather than the component's. The guest sets
            // `Content-Length`, so this arm is the one that would catch it *stopping*.
            None if head.contains("transfer-encoding: chunked") => loop {
                let mut line = Vec::new();
                while !line.ends_with(b"\r\n") {
                    match socket.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => line.push(byte[0]),
                    }
                }
                let size =
                    usize::from_str_radix(String::from_utf8_lossy(&line).trim(), 16).unwrap_or(0);
                let mut chunk = vec![0u8; size + 2];
                if socket.read_exact(&mut chunk).is_err() {
                    break;
                }
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..size]);
            },
            None => {}
        }
        let _ = sender.send(String::from_utf8_lossy(&request).into_owned());

        let _ = socket.write_all(&answer);
        let _ = socket.flush();
    });

    Peer { address, seen }
}

#[test]
fn the_client_posts_ecma_431_section_six_and_reads_the_reply_back() {
    use nlipsy_conformance::component::{Wire, block_on, context_of};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    let answered = Message::text("A standards body.")
        .with_submessage(Submessage::token("conversation", "c-1"))
        .with_submessage(Submessage::token("authentication_srv-7", "a-7"));
    let peer = one_shot(200, &answered);
    let asked = Message::text("What is Ecma?");

    let reply = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            let url = format!("http://{}/nlip", peer.address);
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call(
                        accessor,
                        url,
                        message_out(&asked),
                        Wire::Json,
                        context_of(None, 0),
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call`")
        .expect("the end-point answered");

    // The derived views, computed in Rust and carried as fields.
    assert_eq!(reply.text.as_deref(), Some("A standards body."));
    assert_eq!(reply.conversation_tokens, ["c-1"]);
    assert_eq!(reply.authentication_tokens, ["a-7"]);
    assert!(!reply.is_error);
    assert_eq!(reply.bytes as usize, answered.to_json_string().len());

    // And what went out is ECMA-431 §6's request: `POST` on the whole URL the caller
    // gave, `application/json`, and the body `encode_request` wrote.
    let sent = peer.seen.recv().expect("the end-point saw a request");
    let lower = sent.to_lowercase();
    assert!(lower.starts_with("post /nlip "), "{sent}");
    assert!(
        lower.contains(&format!("content-type: {}", nlipsy_http::CONTENT_TYPE)),
        "{sent}"
    );
    assert!(
        lower.contains(&format!("content-length: {}", asked.to_json_string().len())),
        "the length is known before the request goes out, so the request is framed \
         rather than chunked — which is what lets a peer apply its own body limit \
         before reading a byte: {sent}"
    );
    assert!(sent.ends_with(&asked.to_json_string()), "{sent}");
}

#[test]
fn the_client_carries_the_contexts_correlator_in_this_bindings_header() {
    use nlipsy_conformance::component::{Wire, block_on};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    let peer = one_shot(200, &Message::text("ok"));

    let cx = nlipsy_conformance::component::WitContext {
        peer: None,
        correlator: Some("corr-out".to_owned()),
        exchange: 0,
    };
    let _ = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            let url = format!("http://{}/nlip", peer.address);
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call(
                        accessor,
                        url,
                        message_out(&Message::text("hi")),
                        Wire::Json,
                        cx,
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call`")
        .expect("the end-point answered");

    let sent = peer.seen.recv().expect("the end-point saw a request");
    assert!(
        sent.to_lowercase()
            .contains(&format!("{}: corr-out", nlipsy_http::CORRELATOR_HEADER)),
        "the concept lives in the context and each binding spells it: {sent}"
    );
}

#[test]
fn a_recording_client_writes_what_passed_through_to_the_host_sink() {
    use nlipsy_conformance::component::{Wire, block_on, context_of};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    let answered = Message::text("hello");
    let peer = one_shot(200, &answered);
    let asked = Message::text("hi");

    let reply = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            let url = format!("http://{}/nlip", peer.address);
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call_recording(
                        accessor,
                        url,
                        message_out(&asked),
                        Wire::Json,
                        context_of(None, 0),
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call-recording`")
        .expect("the end-point answered");

    assert_eq!(reply.text.as_deref(), Some("hello"));

    // `Recorded::to_json_line`'s spelling, unchanged — which is what lets a fixture
    // recorded here load in `client.replay` and the other way round.
    let lines = &instance.host().sink.lines;
    assert_eq!(lines.len(), 1, "one exchange, one line");
    let recorded: serde_json::Value =
        serde_json::from_str(&lines[0]).expect("`Recorded::to_json_line` is JSON");
    assert_eq!(
        recorded["request"].as_str(),
        Some(asked.to_json_string().as_str()),
        "the request as it went out, so a fixture pins the client's encoding too"
    );
    assert_eq!(recorded["status"].as_u64(), Some(200));
    assert_eq!(
        recorded["body"].as_str(),
        Some(answered.to_json_string().as_str())
    );
}

#[test]
fn a_url_that_is_not_an_absolute_http_url_is_unreachable_and_nothing_is_sent() {
    use nlipsy_conformance::component::{TransportError, Wire, block_on, context_of};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));

    let failed = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call(
                        accessor,
                        "not-a-url".to_owned(),
                        message_out(&Message::text("hi")),
                        Wire::Json,
                        context_of(None, 0),
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call`")
        .expect_err("there is nowhere to send this");

    // `unreachable` and not one of the other two: nothing was sent, so the end-point
    // cannot have seen the request and a caller may repeat it freely.
    match failed {
        TransportError::Unreachable(failure) => {
            assert_eq!(failure.endpoint, "not-a-url");
            assert!(!failure.detail.is_empty());
        }
        other => panic!("expected `unreachable`, got {other:?}"),
    }
}

#[test]
fn a_reply_body_that_ends_early_is_interrupted_and_not_a_successful_exchange() {
    use nlipsy_conformance::component::{TransportError, Wire, block_on, context_of};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    let peer = one_shot_truncated(&Message::text(
        "A standards body, and this answer is long enough to be worth cutting in half.",
    ));

    let url = format!("http://{}/nlip", peer.address);
    let failed = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            let url = url.clone();
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call(
                        accessor,
                        url,
                        message_out(&Message::text("What is Ecma?")),
                        Wire::Json,
                        context_of(None, 0),
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call`")
        .expect_err("the reply did not arrive");

    // `interrupted` and not one of the other two: the headers arrived, so the end-point
    // saw the request, and whether it acted on it is genuinely unknown — which is the
    // whole reason `transport-error` has three cases. Reported as a *successful*
    // exchange with an opaque body, a caller would read "answered 200 with something
    // that is not NLIP" and would not retry.
    match failed {
        TransportError::Interrupted(failure) => {
            assert_eq!(failure.endpoint, url);
            assert!(
                failure
                    .detail
                    .contains("reply headers arrived but the body did not"),
                "the reqwest binding's own sentence \
                 (`crates/nlipsy-http-client/src/transport.rs:204–210`): {}",
                failure.detail
            );
        }
        other => panic!("expected `interrupted`, got {other:?}"),
    }
}

#[test]
fn the_clients_retry_after_is_parsed_and_a_non_2xx_reply_is_carried() {
    use nlipsy_conformance::component::{Wire, block_on, context_of};

    let mut instance = Instance::load().unwrap_or_else(|why| panic!("{why}"));
    let peer = one_shot_with(
        503,
        &Message::error("the index is rebuilding"),
        &[("Retry-After", "120")],
    );

    let reply = instance
        .enter(|store, engine| {
            let client = engine.nlipsy_engine_client();
            let url = format!("http://{}/nlip", peer.address);
            block_on(store.run_concurrent(async |accessor| {
                client
                    .call_call(
                        accessor,
                        url,
                        message_out(&Message::text("hi")),
                        Wire::Json,
                        context_of(None, 0),
                    )
                    .await
            }))
        })
        .expect("run_concurrent")
        .expect("`client.call`")
        .expect("a non-2xx reply is an answer this transport carried, not a failure");

    // RFC 9110 §10.2.3's delay-seconds form, in the milliseconds this boundary carries
    // durations in — `parse_retry_after`, the same function the reqwest binding calls.
    let nlipsy_conformance::component::ExchangeDetail::Http(detail) = reply.detail;
    assert_eq!(detail.status, 503);
    assert_eq!(detail.retry_after_ms, Some(120_000));
    // And the body is still decoded and rendered: a non-2xx is carried rather than
    // turned into an error of the transport's, which is what `ExchangeDetail::is_ok`
    // exists for.
    assert!(reply.is_error);
    assert_eq!(reply.text.as_deref(), Some("the index is rebuilding"));
}

// ---------------------------------------------------------------------------
// The composition: the engine plugged into a Rust end-point, and no host at all
// ---------------------------------------------------------------------------

/// One request through the composed artefact — the proof that a deployment needs no host
/// language.
///
/// `examples/echo-endpoint` supplies the five `host-*` interfaces, `wac plug` joins them
/// to the engine's imports, and what is left imports only `wasi:*`. This drives it the
/// way `wasmtime serve` does, minus the socket: nothing of this harness's reaches the
/// answer, so what comes back is the protocol's and the deployment's and nothing else.
#[test]
fn the_composed_artefact_answers_a_post_with_no_host_of_its_own() {
    use nlipsy_conformance::component::Composed;

    let mut composed = Composed::load().unwrap_or_else(|why| panic!("{why}"));

    let asked =
        Message::text("What is Ecma?").with_submessage(Submessage::token("conversation", "cli-1"));
    let answer = composed
        .http(
            "POST",
            "http://127.0.0.1/nlip",
            &[
                ("content-type", "application/json"),
                (nlipsy_http::CORRELATOR_HEADER, "demo-1"),
            ],
            asked.to_json_vec(),
        )
        .expect("the composition answered");

    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.header("content-type"),
        Some(nlipsy_http::CONTENT_TYPE)
    );
    assert_eq!(
        answer.header(nlipsy_http::CORRELATOR_HEADER),
        Some("demo-1")
    );

    let reply = body_of(&answer);
    // The end-point's own answer: `extract_text` over the request, echoed back.
    assert_eq!(reply.content_as_str(), Some("What is Ecma?"));
    // And clause 6's, which the end-point knows nothing about: §6.2 ¶2 put the peer's
    // token back on a reply that never carried one.
    let echoed: Vec<&str> = reply
        .tokens()
        .filter_map(Submessage::content_as_str)
        .collect();
    assert_eq!(echoed, ["cli-1"]);
}

/// And the same composition answers a method mismatch in the protocol.
///
/// The binding is the engine's, so a deployment that wrote no HTTP still gets `405`,
/// `Allow: POST` and an NLIP body — which is the whole claim of "the host supplies code,
/// not protocol".
#[test]
fn the_composed_artefact_answers_a_method_mismatch_without_the_deployment_knowing() {
    use nlipsy_conformance::component::Composed;

    let mut composed = Composed::load().unwrap_or_else(|why| panic!("{why}"));
    let answer = composed
        .http("GET", "http://127.0.0.1/nlip", &[], Vec::new())
        .expect("the composition answered");

    assert_eq!(answer.status, 405);
    assert_eq!(answer.header("allow"), Some("POST"));
    assert_eq!(body_of(&answer), nlipsy_http::method_not_allowed());
}
