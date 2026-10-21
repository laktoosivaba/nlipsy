//! Turning a message into an HTTP response, and a refusal into one too.
//!
//! The principle the reference server stated and this crate inherits: *a request that
//! cannot be understood is answered with a non-2xx status **and** an NLIP
//! `MessageType: Error` body, so a peer reading only the payload and one reading only
//! the status line both learn what happened.* Every refusal this crate can produce
//! goes through here, which is what makes that a property rather than an intention.

use std::time::Duration;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use nlipsy_protocol::message::Message;

// The media type this binding writes on a reply — ECMA-430 §5 ¶2's encoding at
// ECMA-433 §6.1.5's media type, since ECMA-431 §6 names neither. It is `nlipsy-http`'s,
// because the client binding writes the same header on its request, and re-exported
// here so that the path a consumer already wrote keeps working.
pub use nlipsy_http::CONTENT_TYPE;

/// An NLIP error reply for a deployment's own layer, rendered before there is
/// anything to echo.
///
/// A rate limiter, a host filter or an authentication layer refuses a request that
/// this crate has not decoded and may never decode — there is no `Message` to take
/// tokens from and no [`Context`](nlipsy_core::Context) to ask a
/// [`TokenAuthority`](nlipsy_core::TokenAuthority) about. What such a layer still
/// owes the peer is a reply in the protocol rather than a bare status line, and this
/// is it: the same body shape, the same content type, at whatever status the layer
/// chose.
///
/// §6.2 cannot be honoured on such a refusal — there was nothing to take a token
/// from — and §6.1 still is: what goes back is a message with a `text` first
/// submessage. A peer refused here should treat its state as untouched, because
/// nothing of what it sent was consumed.
///
/// `retry_after` becomes a `Retry-After` header in seconds, floored at 1 — a header
/// saying "come back in zero seconds" is an invitation to a tight loop.
///
/// The caller may still add headers to the returned response; a deployment that wants
/// its correlator on the refusal calls
/// [`ServerConfig::echo_correlator`](crate::ServerConfig::echo_correlator) on it.
pub fn reject(status: StatusCode, text: &str, retry_after: Option<Duration>) -> Response {
    json_response(status, &Message::error(text), retry_after)
}

/// One message, encoded, at one status.
///
/// The body is written by `nlipsy-protocol` rather than by a serde derive on a type
/// of this crate's: clause 5 fixes the field order and which fields are omitted, and
/// only the crate that models the message knows that.
pub(crate) fn json_response(
    status: StatusCode,
    message: &Message,
    retry_after: Option<Duration>,
) -> Response {
    let body = message.to_json_vec();
    // The header array is applied after the body's own `IntoResponse`, so it replaces
    // the `application/octet-stream` a `Vec<u8>` would otherwise carry.
    let mut response = (status, [(header::CONTENT_TYPE, CONTENT_TYPE)], body).into_response();
    if let Some(delay) = retry_after {
        // A dependency that named a delay is telling the peer when to come back;
        // passing it on turns a blind retry into a paced one. The spelling — seconds,
        // floored at one — is `nlipsy-http`'s, because the client binding has to read
        // back what this writes.
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, nlipsy_http::retry_after_value(delay));
    }
    response
}
