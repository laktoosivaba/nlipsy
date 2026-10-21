//! `export wasi:http/handler@0.3.0` — ECMA-431 §6, in the guest.
//!
//! The one place a peer's bytes enter and the one place they leave. It is
//! `nlipsy-server`'s behaviour over axum, transposed: the same crate underneath
//! (`nlipsy-http`), the same principle — *a request that cannot be understood is
//! answered with a non-2xx status **and** an NLIP `MessageType: Error` body, so a peer
//! reading only the payload and one reading only the status line both learn what
//! happened* — and the same values, read once per request from
//! `host-endpoint.http-binding` rather than from a `ServerConfig`.
//!
//! ```text
//! POST  /nlip[/] → body under the limit → engine.serve → StatusMap::of, Retry-After,
//!                  Content-Type, the correlator echoed or minted
//! other /nlip[/] → 405, Allow: POST, method_not_allowed()
//! OPTIONS /nlip[/] → 204 with the cross-origin headers, but only under http-options.cors
//!                  and only for an Origin that matches; otherwise the 405 above
//! any other path → 404 with an NLIP error body
//! ```
//!
//! **Both spellings of the path reach the end-point**, `/nlip` and `/nlip/` alike, which
//! is the axum router's own arrangement and for its stated reason: the ECMA-430
//! reference server mounts the trailing-slash spelling while the reference client POSTs
//! to the other (`crates/nlipsy-server/src/server.rs:402–406`). See [`serves`].
//!
//! ECMA-431 §6 fixes two things and no more — the path and the method — so every status
//! code, header name and limit below is a *value* the host configured, and the only
//! literals here are the ones the standard itself names: `POST` in `Allow`, and the
//! `404`/`405` a path and a method mismatch get.
//!
//! The upload leg of §6.1 is **not** here. ECMA-431 makes it a "may", and adding it is
//! additive: a host `upload-sink` import and a route, with the status vocabulary
//! `StatusMap::of_error` already supplies.

use nlipsy_http::{
    CONTENT_TYPE, CORRELATOR_HEADER, NLIP_PATH, method_not_allowed, parse_retry_after,
    retry_after_value, too_large,
};
use nlipsy_protocol::{Message, Wire};

use crate::Component;
use crate::bindings::exports::wasi::http::handler::Guest;
use crate::bindings::nlipsy::engine::host_endpoint;
use crate::bindings::nlipsy::engine::types as wt;
use crate::bindings::wasi::http::client;
use crate::bindings::wasi::http::types::{ErrorCode, Fields, Method, Request, Response, Scheme};
use crate::bindings::wasi::random::random;
use crate::bindings::{wit_future, wit_stream};
use crate::convert::status_map_in;

/// The largest correlator this binding will echo, in bytes.
///
/// **128, and it is `nlipsy-server`'s number** — `MAX_CORRELATOR_LENGTH`
/// (`crates/nlipsy-server/src/server.rs:55`) — taken rather than re-decided, because the
/// two bindings are twins everywhere else and a peer that moves between them should not
/// find one of them stricter. It is a copy and not an import: that crate is axum over
/// hyper over tokio and does not compile for `wasm32-wasip3`.
///
/// The rule is **this binding's and not the concept's**.
/// ECMA-431 §6 names no correlator at all — the header is a convention — while ECMA-433
/// §6.1.4 makes copying the AMQP `correlation-id` normative and names no length, so an
/// AMQP binding must not carry the guard over. Above the bound the peer's value is
/// discarded and a fresh one minted, and the substitution is *visible*: the reply carries
/// the fresh value, so a client that sent an over-long correlator sees the correlation
/// fail rather than quietly match the wrong exchange.
const MAX_CORRELATOR_LENGTH: usize = 128;

/// The status a path this end-point does not serve gets.
///
/// Not in `status-map`, and correctly: no `outcome` produces a path mismatch, so there is
/// nothing for a deployment to move it *from*. The body is the binding's one principle
/// applied — every reply is an NLIP `MessageType: Error` message, a path nobody serves
/// included, so a peer never has to parse a second body shape.
const NOT_FOUND: u16 = 404;

/// The status another method on the end-point's own path gets.
///
/// ECMA-431 §6 obliges `POST` and forbids nothing, so this is the binding's choice like
/// every other; it is a literal for the same reason `NOT_FOUND` is — no `outcome`
/// produces a method mismatch, so `status-map` has no field to hold it — and not an
/// option.
const METHOD_NOT_ALLOWED: u16 = 405;

/// The status a matched cross-origin preflight gets: no content, and the headers are the
/// answer.
const NO_CONTENT: u16 = 204;

impl Guest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        // Read before the request is consumed: `consume-body` takes it by value.
        let options = host_endpoint::http_binding();
        let method = request.get_method();
        let target = request.get_path_with_query().unwrap_or_default();
        let headers = request.get_headers();
        let sent_correlator = options
            .correlator_header
            .as_deref()
            .and_then(|name| header(&headers, name));
        let origin = header(&headers, "origin");

        let path = path_of(&target);
        let cross_origin = allowed_origin(options.cors.as_ref(), origin.as_deref());

        // Every path drains the incoming body, whatever it does with it: the request
        // resource has to be released, and a peer that sent bytes to the wrong path is
        // owed the same courtesy as one that sent them to the right one.
        let limit = options.max_request_body_size as usize;
        let body = read_body(request, limit).await;

        if !serves(path) {
            return Ok(answer(
                NOT_FOUND,
                &Message::error(format!("NLIP messages are POSTed to {NLIP_PATH}")),
                None,
                correlator_for(sent_correlator),
                options.correlator_header.as_deref(),
                &cross_origin,
            ));
        }

        if matches!(method, Method::Options) {
            // Only under `http-options.cors`, and only for an `Origin` that matches:
            // without both, `OPTIONS` is just another method on the path (the rule
            // `allowed_origin` states).
            if cross_origin.is_some() {
                return Ok(answer(
                    NO_CONTENT,
                    &Message::error("preflight"),
                    None,
                    correlator_for(sent_correlator),
                    options.correlator_header.as_deref(),
                    &cross_origin,
                ));
            }
        }

        if !matches!(method, Method::Post) {
            return Ok(answer(
                METHOD_NOT_ALLOWED,
                &method_not_allowed(),
                None,
                correlator_for(sent_correlator),
                options.correlator_header.as_deref(),
                &cross_origin,
            ));
        }

        let statuses = status_map_in(options.status);
        let correlator = correlator_for(sent_correlator);

        let body = match body {
            Ok(body) => body,
            // The limit is enforced *while* the body is read, so this is reached before
            // there are bytes to decode: §6.2 cannot be honoured — the body was never
            // decoded, so no token was ever read — and §6.1 still is, because what goes
            // back is an NLIP message with a `text` first submessage. It is also the one
            // refusal after which a peer should treat its state as untouched: nothing it
            // sent was consumed.
            Err(BodyFailure::TooLarge) => {
                return Ok(answer(
                    statuses.too_large.as_u16(),
                    &too_large(limit),
                    None,
                    correlator,
                    options.correlator_header.as_deref(),
                    &cross_origin,
                ));
            }
            // The peer never finished sending, so there is no request to run clause 6
            // over: answering `200` with a full reply would record a turn from a message
            // that was never delivered, and blaming the peer for a truncated JSON
            // document would name the wrong party. `status.internal` and "the request
            // body could not be read" are the axum binding's own answer, word for word
            // (`Server::unreadable_body`, `crates/nlipsy-server/src/server.rs:528–546`),
            // and the sentence goes out through the engine's `error_reply` rather than
            // through a second `Message::error` written here. No `Retry-After`: nothing
            // about a broken connection says when to come back.
            Err(BodyFailure::Broken(detail)) => {
                // The `error-code` itself is the operator's and not the peer's —
                // ECMA-434 §7.3.3 is why `internal` says nothing more than the sentence,
                // and the axum binding logs the error and answers with that sentence
                // alone. A component has no logger of its own, so the value is named
                // here and dropped rather than discarded silently at the call.
                drop(detail);
                let broken = crate::host::context_in(wt::Context {
                    peer: None,
                    correlator: Some(correlator.clone()),
                    exchange: 0,
                });
                return Ok(answer(
                    statuses.internal.as_u16(),
                    &nlipsy_core::error_reply(
                        &crate::host::steps(),
                        None,
                        "the request body could not be read",
                        nlipsy_core::Owed::Nothing,
                        &broken,
                    ),
                    None,
                    correlator,
                    options.correlator_header.as_deref(),
                    &cross_origin,
                ));
            }
        };

        // The request's own `Content-Type` is not read, and that is deliberate: ECMA-431
        // §6 fixes the URL and the method and names no media type, so this binding has
        // none to enforce, and a peer that sends JSON under `text/plain` has sent a
        // message — which is exactly what `crates/nlipsy-server/src/server.rs` says at
        // the same place. What comes back does carry one.
        //
        // `Wire::Json` and not a choice: ECMA-431 §6 carries JSON (ECMA-430 §5 ¶2). A
        // frame-based binding (ECMA-432 §7.1) passes `Wire::Cbor` to the same composer.
        // `peer` is `none`, and that is a limit of the runtime rather than an omission
        // here: `wasi:http@0.3.0`'s `request` exposes the method, the scheme, the
        // authority, the path and the headers, and no remote address at all — so there
        // is nothing to fill it from and a fabricated label would be worse than `none`,
        // which a host can at least read as "this binding does not know". The axum
        // binding does have one and fills it, from `ConnectInfo`
        // (`crates/nlipsy-server/src/server.rs:219–231`). A deployment behind a proxy
        // that needs a peer reads its own forwarded header in a layer of its own: that
        // rule is the deployment's, ECMA-431 §6 names no such header, and inventing one
        // here would be an export with no seed (`wit/README.md` §1).
        let cx = wt::Context {
            peer: None,
            correlator: Some(correlator.clone()),
            exchange: 0,
        };
        let served = crate::engine::serve_bytes(body, Wire::Json, cx).await;

        let status = statuses.of(&served.outcome, served.withheld).as_u16();
        let retry_after = served.outcome.retry_after();
        Ok(answer(
            status,
            &served.reply,
            retry_after.map(|delay| {
                // Whole seconds, floored at one and never zero: a header saying "come
                // back in zero seconds" is an invitation to a tight loop. The crate hands
                // back `http`'s own type because a Rust binding puts it in a `HeaderMap`;
                // this one writes the string, and the value is ASCII digits, so `to_str`
                // cannot fail — the fallback is that same floor rather than a panic.
                retry_after_value(delay).to_str().unwrap_or("1").to_owned()
            }),
            correlator,
            options.correlator_header.as_deref(),
            &cross_origin,
        ))
    }
}

/// The first value of `name`, as text.
///
/// A header value is bytes at this boundary; one that is not UTF-8 is not a correlator
/// and not an origin either, so it is read as absent rather than replaced with something
/// lossy.
fn header(headers: &Fields, name: &str) -> Option<String> {
    let values = headers.get(name);
    let first = values.into_iter().next()?;
    String::from_utf8(first).ok()
}

/// The path, without the query or the fragment.
///
/// `get-path-with-query` is what the name says, and ECMA-431 §6 fixes a *path*; a peer
/// that appends a query to `/nlip` has still reached the end-point.
fn path_of(target: &str) -> &str {
    let cut = target.find(['?', '#']).unwrap_or(target.len());
    &target[..cut]
}

/// Whether this binding serves `path`.
///
/// **Both spellings**, `/nlip` and `/nlip/`, which is what the axum router mounts and
/// for the reason written on it: "Both spellings, because the reference client POSTs to
/// `/nlip` while the reference server mounts `/nlip/`; serving both avoids depending on
/// a redirect, and on which of the two a given client's HTTP stack follows"
/// (`crates/nlipsy-server/src/server.rs:402–406`). ECMA-431 §6 fixes the path and says
/// nothing about the slash, so the only question is whether the workspace's two bindings
/// answer a peer the same way, and this is that answer.
///
/// The slash is *stripped* rather than a second literal appended: [`NLIP_PATH`] stays
/// the one place the path is written, and nothing is allocated per request.
fn serves(path: &str) -> bool {
    path.strip_suffix('/').unwrap_or(path) == NLIP_PATH
}

/// The correlator this exchange is answered under.
///
/// The peer's value when it is sane — non-empty and at most [`MAX_CORRELATOR_LENGTH`]
/// **bytes**, which is what `ServerConfig::correlator_for`
/// (`crates/nlipsy-server/src/server.rs:156`) measures too — bytes and not characters,
/// so that a multibyte value is judged the same by both bindings. Otherwise a fresh one,
/// minted here.
fn correlator_for(sent: Option<String>) -> String {
    match sent {
        Some(value) if usable(&value) => value,
        _ => mint_correlator(),
    }
}

/// Whether a correlator the peer sent may be echoed.
///
/// The rule, separated from the minting so that it is testable without a host: reading
/// randomness is a `wasi:random` call and everything around it here is arithmetic on a
/// string.
fn usable(sent: &str) -> bool {
    // Visible ASCII, which is the axum binding's rule rather than a new one: it filters
    // the header through `HeaderValue::to_str`
    // (`crates/nlipsy-server/src/server.rs:156`), and `http`'s `to_str` is exactly RFC
    // 9110 §5.5's field-value grammar — visible characters, plus the space and the
    // horizontal tab. This binding read any UTF-8 and echoed it, so `café-1` came back
    // from the component and was replaced by a fresh UUID by axum; the bound above is
    // taken from that crate for the same reason, so the rule around it is taken too.
    let spelling = |byte: u8| (0x21..=0x7e).contains(&byte) || byte == b' ' || byte == b'\t';
    !sent.is_empty() && sent.len() <= MAX_CORRELATOR_LENGTH && sent.bytes().all(spelling)
}

/// A fresh correlator: a UUID version 4, in the spelling `Uuid::new_v4().to_string()`
/// produces.
///
/// The bytes are `wasi:random/random@0.3.0`'s, which the world imports by name because
/// `std` does not link it on `wasm32-wasip3`. It is written out here rather than taken
/// from the `uuid` crate for one reason: `uuid`'s `v4` reaches for `getrandom`, and what
/// this component has is the WASI interface — so the dependency would buy a formatter and
/// a random source this cannot use.
///
/// RFC 9562 §4.4: sixteen random bytes, with the version nibble set to `4` and the
/// variant bits to `10`.
fn mint_correlator() -> String {
    let mut bytes = random::get_random_bytes(16);
    // A host that answered short is not something the interface allows, and inventing
    // entropy would be worse than a shorter identifier: pad with what is there rather
    // than index past the end.
    bytes.resize(16, 0);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The origin this reply may name, or `none` for no cross-origin headers at all.
///
/// **The one rule.** Nothing is emitted unless the request carried an
/// `Origin` *and* that origin matches: a `*` beside
/// `Access-Control-Allow-Credentials: true` is a pair no browser accepts, and a
/// deployment reading its own responses in `curl` should never see one. A configured `*`
/// still answers a request that sent an origin — with the origin itself when credentials
/// are on, because that is the only form a credentialed request accepts.
fn allowed_origin(cors: Option<&wt::Cors>, origin: Option<&str>) -> Option<(String, bool)> {
    let cors = cors?;
    let origin = origin?;
    let matched = cors
        .origins
        .iter()
        .any(|allowed| allowed == "*" || allowed == origin);
    if !matched {
        return None;
    }
    Some((origin.to_owned(), cors.credentials))
}

/// Read the request's body, refusing at `limit` bytes.
///
/// The limit is applied **while the body is read** — never buffering past it — which is
/// the whole reason it is here and not in a framework layer around the component: a
/// framework that refused in its own voice would answer an NLIP request with a plain-text
/// error, and `nlipsy-server` moves the same check inside for the same reason
/// (`crates/nlipsy-server/src/server.rs`, `ServerConfig::max_request_body_size`).
///
/// [`BodyFailure::TooLarge`] means the peer sent more than `limit`. The reader is
/// dropped there, which cancels the rest of the stream.
/// [`BodyFailure::Broken`] means the peer never finished sending: the stream ended and
/// the trailers future — the only place `wasi:http` reports a body-stream failure —
/// resolved to an `error-code`.
async fn read_body(request: Request, limit: usize) -> Result<Vec<u8>, BodyFailure> {
    let (done, on_done) = wit_future::new(|| Ok(()));
    // The second value is **kept**. `stream<u8>` answers `Dropped` for a clean end and
    // for a broken one alike, so the stream alone cannot tell "the peer finished" from
    // "the connection died"; `future<result<option<trailers>, error-code>>` is where
    // `wasi:http@0.3.0` says which it was (`wit/deps/http.wit:343`), and dropping it is
    // how a request the peer never finished sending gets answered as if it had.
    //
    // `done` is a different value and is dropped on the next line as it always was:
    // that is the *result* future this reader owes the request, not the trailers.
    let (mut body, trailers) = Request::consume_body(request, on_done);
    drop(done);

    let mut read: Vec<u8> = Vec::new();
    loop {
        // One byte of headroom over the limit and no more, so that "the peer sent too
        // much" is observable without the bytes past it ever being held. `reserve_exact`
        // and not `reserve`: the amortised one takes `max(cap * 2, len + additional)`,
        // so near the limit it doubles past it, `StreamReader::read` fills every byte of
        // the spare capacity, and the check below fires only once those bytes are in
        // memory — a 2.00x overshoot of the bound this field exists to impose
        // (measured: 2.00x with `reserve`, 1.00x with `reserve_exact`).
        if read.capacity() <= read.len() {
            let room = (limit + 1).saturating_sub(read.len()).min(64 * 1024);
            if room == 0 {
                return Err(BodyFailure::TooLarge);
            }
            read.reserve_exact(room);
        }
        let (status, buffer) = body.read(read).await;
        read = buffer;
        if read.len() > limit {
            return Err(BodyFailure::TooLarge);
        }
        match status {
            wit_bindgen::StreamResult::Complete(_) => {}
            wit_bindgen::StreamResult::Dropped => break,
            // Only a cancelled read reports this, and nothing here cancels one.
            wit_bindgen::StreamResult::Cancelled => break,
        }
    }

    // The stream is closed, which is the point the interface says this future resolves
    // at: `Ok` means every byte the peer meant to send arrived.
    match trailers.await {
        Ok(_) => Ok(read),
        Err(code) => Err(BodyFailure::Broken(format!("{code:?}"))),
    }
}

/// Why a request body is not a body.
///
/// Two failures and not one, because they are a `413` and a `500` and they say opposite
/// things about whose fault it is — which is the distinction `Server::unreadable_body`
/// draws on the axum side (`crates/nlipsy-server/src/server.rs:528–546`).
#[derive(Debug)]
enum BodyFailure {
    /// The peer sent more than `http-options.max-request-body-size`.
    TooLarge,
    /// The peer never finished sending. The detail is the `error-code`, for a log line.
    Broken(String),
}

/// One NLIP message, encoded, at one status, with the headers this binding writes.
///
/// Every reply goes through here, which is what makes "a non-2xx status **and** an NLIP
/// body" a property rather than an intention. The body is written by `nlipsy-protocol`
/// rather than by a serializer of this crate's: clause 5 fixes the field order and which
/// fields are omitted, and only the crate that models the message knows that.
fn answer(
    status: u16,
    message: &Message,
    retry_after: Option<String>,
    correlator: String,
    correlator_header: Option<&str>,
    cross_origin: &Option<(String, bool)>,
) -> Response {
    // Encoded before the headers, because one of the headers is its length.
    let payload = if status == NO_CONTENT {
        Vec::new()
    } else {
        message.to_json_vec()
    };

    let headers = Fields::new();
    // The only way any of these fails is a header name or value the standard forbids, and
    // every name here is a constant while every value is ASCII this binding produced; a
    // reply missing a header is still a reply, so a failure is dropped rather than turned
    // into an `ErrorCode` the peer cannot act on.
    let _ = headers.append("content-type", CONTENT_TYPE.as_bytes());
    if status != NO_CONTENT {
        // The length is known — the message is encoded above — so the reply goes out
        // framed rather than chunked. It is the argument this file's own client half
        // already makes for the request (`http.rs:569–573`): a peer that wants to apply
        // its own limit before reading has
        // nothing to read it from otherwise, and a *client* bounding a reply has the same
        // problem. The axum binding answers it the same way — `json_response`
        // (`crates/nlipsy-server/src/reply.rs:52–71`) builds the response from a `Vec<u8>`
        // whose `size_hint` is exact, so hyper writes the header — and two bindings of
        // one clause should not frame a reply differently.
        //
        // `204` gets none: a preflight has no body, and a `content-length` beside no body
        // is a header RFC 9110 §8.6 does not want.
        let _ = headers.append("content-length", payload.len().to_string().as_bytes());
    }
    if status == METHOD_NOT_ALLOWED {
        // The method ECMA-431 §6 names. It has no export and no seed, and inventing a
        // constant for it would be an export with neither.
        let _ = headers.append("allow", b"POST");
    }
    if let Some(name) = correlator_header {
        let _ = headers.append(name, correlator.as_bytes());
    }
    if let Some(delay) = retry_after {
        let _ = headers.append("retry-after", delay.as_bytes());
    }
    if let Some((origin, credentials)) = cross_origin {
        let _ = headers.append("access-control-allow-origin", origin.as_bytes());
        let _ = headers.append("vary", b"Origin");
        let _ = headers.append("access-control-allow-methods", b"POST, OPTIONS");
        let mut allowed = "content-type".to_owned();
        if let Some(name) = correlator_header {
            allowed.push_str(", ");
            allowed.push_str(name);
        }
        let _ = headers.append("access-control-allow-headers", allowed.as_bytes());
        if *credentials {
            let _ = headers.append("access-control-allow-credentials", b"true");
        }
    }

    let (mut sink, source) = wit_stream::new();
    let (trailers, on_trailers) = wit_future::new(|| Ok(None));
    let (response, _transmit) = Response::new(headers, Some(source), on_trailers);
    drop(trailers);
    let _ = response.set_status_code(status);

    // The body is a `stream<u8>` the guest writes *after* the `Response` has been
    // returned, so the writer is a task of its own: `wasi:http@0.3.0`'s shape, and the
    // one place this crate needs `wit_bindgen::spawn_local`.
    wit_bindgen::spawn_local(async move {
        let remaining = sink.write_all(payload).await;
        debug_assert!(remaining.is_empty());
    });
    response
}

// ---------------------------------------------------------------------------
// The outgoing half: `wasi:http/client@0.3.0`, behind the engine's `Transport`
// ---------------------------------------------------------------------------

/// One end-point, reached over `wasi:http/client@0.3.0` — the client half of ECMA-431 §6.
///
/// It is a [`Transport`](nlipsy_core::Transport), so `Client::send`
/// (`crates/nlipsy-core/src/transport.rs:576`) is what encodes, carries and decodes, and
/// this type is only the *binding*: `POST url` with `CONTENT_TYPE`, the correlator from
/// the exchange's context in the header the deployment named, and the reply's status and
/// `Retry-After` read back into an `ExchangeDetail::Http`.
///
/// `url` is the whole URL a deployment published, not a host and a port with `NLIP_PATH`
/// appended: an end-point may sit behind a prefix a caller cannot derive, which is what
/// that constant's own doc says.
#[derive(Debug)]
pub struct Outgoing {
    url: String,
}

impl Outgoing {
    /// Talk to the end-point at `url`.
    pub fn to(url: String) -> Outgoing {
        Outgoing { url }
    }
}

impl nlipsy_core::Transport for Outgoing {
    async fn exchange(
        &self,
        cx: &nlipsy_core::Context,
        body: Vec<u8>,
    ) -> Result<nlipsy_core::Exchange, nlipsy_core::TransportError> {
        let Some((scheme, authority, path)) = split_url(&self.url) else {
            return Err(nlipsy_core::TransportError::Unreachable {
                endpoint: self.url.clone(),
                detail: "the URL is not an absolute http or https URL".to_owned(),
            });
        };

        let headers = Fields::new();
        let _ = headers.append("content-type", CONTENT_TYPE.as_bytes());
        // The length is known — the engine encoded the message before this was called —
        // so it is written, and the request goes out framed rather than chunked. That is
        // what `nlipsy-http-client` does over reqwest, and it is what lets a peer apply
        // its own `max-request-body-size` before reading a byte.
        let _ = headers.append("content-length", body.len().to_string().as_bytes());
        if let Some(correlator) = cx.correlator.as_deref() {
            // The header this binding carries a correlator in. The concept lives in the
            // context and each binding spells it; this is HTTP's spelling.
            let _ = headers.append(CORRELATOR_HEADER, correlator.as_bytes());
        }

        let (mut sink, source) = wit_stream::new();
        let (trailers, on_trailers) = wit_future::new(|| Ok(None));
        let (request, _transmit) = Request::new(headers, Some(source), on_trailers, None);
        drop(trailers);
        let _ = request.set_method(&Method::Post);
        let _ = request.set_scheme(Some(&scheme));
        let _ = request.set_authority(Some(&authority));
        let _ = request.set_path_with_query(Some(&path));
        wit_bindgen::spawn_local(async move {
            let remaining = sink.write_all(body).await;
            debug_assert!(remaining.is_empty());
        });

        let response = match client::send(request).await {
            Ok(response) => response,
            // The three cases of `transport-error` differ in what a caller may safely do
            // next, and that is the whole reason they are three. A request that never
            // reached the peer is `unreachable`; one whose reply did not come back is
            // `interrupted`, because whether the end-point acted on it is genuinely
            // unknown and repeating risks the same turn twice in its transcript.
            Err(error) => return Err(self.failed(&error)),
        };

        let status = response.get_status_code();
        let retry_after = header(&response.get_headers(), "retry-after").and_then(|value| {
            // `some(0)` and `none` are different answers: `none` means the end-point said
            // nothing usable, and a caller that reads it as zero will retry immediately
            // against an end-point that never asked it to. The clock is a parameter here
            // for the reason the crate gives — a rule that read it could not be tested
            // against a fixed date — and this binding supplies the one `std` has.
            parse_retry_after(&value, std::time::SystemTime::now())
        });

        let (done, on_done) = wit_future::new(|| Ok(()));
        // The trailers future is kept for the reason `read_body` keeps the incoming one:
        // `StreamReader::collect` reads to `Dropped` and hands back whatever it got, with
        // no error channel of its own, so a reply cut halfway would otherwise be reported
        // as a *successful* exchange whose body happens not to be NLIP — and a caller
        // reading that does not retry. The truth is `interrupted`: whether the end-point
        // acted on the request is unknown, which is the whole reason `transport-error`
        // has three cases. The reqwest binding draws the same line with the same sentence
        // (`crates/nlipsy-http-client/src/transport.rs:204–210`).
        //
        // The body is read with no bound, on this side as on that one:
        // `http-options.max-request-body-size` is the *server's* limit and does not
        // apply to a reply. A `max-reply-body-size` beside it would be additive.
        let (body, trailers) = Response::consume_body(response, on_done);
        let body = body.collect().await;
        drop(done);
        if let Err(code) = trailers.await {
            return Err(nlipsy_core::TransportError::Interrupted {
                endpoint: self.url.clone(),
                detail: format!("reply headers arrived but the body did not: {code:?}"),
            });
        }

        Ok(nlipsy_core::Exchange::new(
            body,
            nlipsy_core::ExchangeDetail::Http {
                status,
                retry_after,
            },
        ))
    }
}

impl Outgoing {
    /// Which of the three a `wasi:http` failure is.
    ///
    /// The split is the one `TransportError`'s own doc draws: a connection that was never
    /// established cannot have been seen by the end-point, so repeating is free; anything
    /// that happened after the request went out leaves that genuinely unknown.
    fn failed(&self, error: &ErrorCode) -> nlipsy_core::TransportError {
        let detail = format!("{error:?}");
        let unreachable = matches!(
            error,
            ErrorCode::DnsTimeout
                | ErrorCode::DnsError(_)
                | ErrorCode::DestinationNotFound
                | ErrorCode::DestinationUnavailable
                | ErrorCode::DestinationIpProhibited
                | ErrorCode::DestinationIpUnroutable
                | ErrorCode::ConnectionRefused
                | ErrorCode::ConnectionTimeout
                | ErrorCode::ConnectionLimitReached
                | ErrorCode::TlsProtocolError
                | ErrorCode::TlsCertificateError
                | ErrorCode::TlsAlertReceived(_)
        );
        if unreachable {
            nlipsy_core::TransportError::Unreachable {
                endpoint: self.url.clone(),
                detail,
            }
        } else {
            nlipsy_core::TransportError::Interrupted {
                endpoint: self.url.clone(),
                detail,
            }
        }
    }
}

/// `http://host:port/path?q` split into the three pieces `wasi:http` asks for.
///
/// `none` for anything that is not an absolute `http` or `https` URL, which a caller
/// learns as `transport-error.unreachable`: nothing was sent, so nothing can be sitting
/// in the peer's transcript.
fn split_url(url: &str) -> Option<(Scheme, String, String)> {
    let (scheme, rest) = match url.split_once("://") {
        Some(("http", rest)) => (Scheme::Http, rest),
        Some(("https", rest)) => (Scheme::Https, rest),
        _ => return None,
    };
    let (authority, path) = match rest.find('/') {
        Some(cut) => (&rest[..cut], &rest[cut..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return None;
    }
    Some((scheme, authority.to_owned(), path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These run on the *host*, through the `rlib`: everything here that does not call a
    // canonical-ABI import is testable without a wasm runtime, and the three rules this
    // binding owns — the path, the correlator bound, the origin rule — are all of that
    // kind.

    #[test]
    fn a_query_string_does_not_move_the_end_point() {
        assert_eq!(path_of("/nlip"), NLIP_PATH);
        assert_eq!(path_of("/nlip?trace=1"), NLIP_PATH);
        assert_eq!(path_of("/nlip#frag"), NLIP_PATH);
        assert_eq!(path_of("/nlip/upload"), "/nlip/upload");
        assert_eq!(path_of(""), "");
    }

    #[test]
    fn both_spellings_of_the_path_reach_the_end_point() {
        // The axum router mounts both and says why
        // (`crates/nlipsy-server/src/server.rs:402–406`); a peer that moves between the
        // two bindings must not find one of them stricter.
        assert!(serves(NLIP_PATH));
        assert!(serves("/nlip/"));
        assert!(serves(path_of("/nlip/?trace=1")));

        // And no further: the upload leg's path is not claimed by accident, and an
        // empty target is not the end-point.
        assert!(!serves("/nlip/upload"));
        assert!(!serves("/nlip//"));
        assert!(!serves("/"));
        assert!(!serves(""));
    }

    #[test]
    fn the_peers_correlator_is_kept_when_it_is_sane_and_replaced_when_it_is_not() {
        // The *rule*, not the minting: `mint_correlator` reads `wasi:random`, which is a
        // canonical-ABI import and an `unreachable!()` stub on this target. What a
        // refused correlator is replaced *with* is asserted through the artefact, in
        // `crates/conformance/tests/component_http.rs`.
        assert!(usable("c-1"));
        assert!(!usable(""), "an empty header value is not an identifier");
        assert!(
            usable(&"x".repeat(MAX_CORRELATOR_LENGTH)),
            "the bound is inclusive, as `ServerConfig::correlator_for`'s is"
        );
        assert!(!usable(&"x".repeat(MAX_CORRELATOR_LENGTH + 1)));

        // Bytes, not characters: the Rust binding measures the header value's bytes and
        // this one measures the same thing.
        let multibyte = "é".repeat(MAX_CORRELATOR_LENGTH);
        assert_eq!(multibyte.chars().count(), MAX_CORRELATOR_LENGTH);
        assert!(!usable(&multibyte));

        // And the character rule, which is `HeaderValue::to_str`'s
        // (`crates/nlipsy-server/src/server.rs:160`) and therefore RFC 9110 §5.5's:
        // anything above 0x7F would otherwise be echoed by this binding and replaced by
        // the axum one, and the two bindings must agree. Short enough to
        // pass the bound, so what refuses it is the spelling and nothing else.
        assert!(
            !usable("café-1"),
            "the byte count is 7, so this is the character rule"
        );
        assert!(!usable("c\u{7f}1"), "DEL is not a visible character");
        assert!(
            !usable("c\n1"),
            "a control character would split the header"
        );
        assert!(usable("c 1"), "a space is inside the field-value grammar");
        assert!(
            usable("c\t1"),
            "and so is a horizontal tab, which `to_str` allows"
        );
    }

    #[test]
    fn no_cross_origin_header_is_emitted_without_an_origin_that_matches() {
        let open = wt::Cors {
            origins: vec!["*".to_owned()],
            credentials: true,
        };
        let named = wt::Cors {
            origins: vec!["https://example.com".to_owned()],
            credentials: false,
        };

        // The origin rule, which is the whole of what this contract promises about CORS: a
        // request that sent no `Origin` gets nothing at all, never `*`.
        assert!(allowed_origin(Some(&open), None).is_none());
        assert!(allowed_origin(None, Some("https://example.com")).is_none());
        assert!(allowed_origin(Some(&named), Some("https://other.example")).is_none());

        // And a match answers with the origin itself rather than with `*`, which is the
        // only form a credentialed request accepts.
        assert_eq!(
            allowed_origin(Some(&open), Some("https://example.com")),
            Some(("https://example.com".to_owned(), true))
        );
        assert_eq!(
            allowed_origin(Some(&named), Some("https://example.com")),
            Some(("https://example.com".to_owned(), false))
        );
    }

    #[test]
    fn a_minted_correlator_is_a_version_4_uuid() {
        // `random::get_random_bytes` is a canonical-ABI import and is an `unreachable!()`
        // stub on this target, so what is checked here is the *formatting* rule over
        // bytes this test supplies — RFC 9562 §4.4's version and variant nibbles, in the
        // spelling `Uuid::new_v4().to_string()` produces.
        let mut bytes: Vec<u8> = (0u8..16).collect();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let spelt = format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        );

        assert_eq!(spelt.len(), 36);
        assert_eq!(spelt.as_bytes()[14], b'4', "the version nibble");
        assert!(
            matches!(spelt.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "the variant bits"
        );
    }
}
