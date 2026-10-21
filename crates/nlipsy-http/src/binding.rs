//! The four constants an ECMA-431 §6 end-point and its caller have to agree on.
//!
//! §6 fixes two things and no more: where the end-point is —
//! "`https://<server_name>:port/nlip`" — and that it accepts a message with `POST`.
//! Everything else below is a *convention* with a reason: the media type comes from
//! ECMA-430 §5 ¶2 and ECMA-433 §6.1.5, the correlator header from the reference
//! deployment, the body limit from ECMA-434 §7.2.5. Each is spelled here once so that
//! the client binding, the server binding and a handler written in another language
//! cannot drift apart on it.

use http::HeaderName;

/// The path ECMA-431 §6 fixes: "This end-point would be accessible using a URL defined
/// as `https://<server_name>:port/nlip`. The end-point shall support accepting an NLIP
/// message using the POST command of HTTP."
///
/// It is not appended to anything by this crate. A client is given the whole URL a
/// deployment published, because an end-point may sit behind a prefix a caller cannot
/// derive; this constant is what a caller composing one from a host and a port uses,
/// and what a server binding mounts.
pub const NLIP_PATH: &str = "/nlip";

/// The header this binding carries a correlator in.
///
/// A convention, not a clause. ECMA-431 defines nothing of the kind; ECMA-433 §6.1.4
/// makes correlation normative over AMQP with a `correlation-id` property — "A server
/// agent that receives a request with a correlation-id **shall** copy the contents …
/// to the correlation-id field of the response". So the *concept* lives in
/// [`Context`](nlipsy_core::Context) and each binding spells it; this is HTTP's
/// spelling, taken from the reference server so that a peer written against it needs no
/// change.
///
/// A `&str` rather than a [`HeaderName`], because the two bindings want it as a
/// different type and one of them is not Rust: a fetch handler writes the string.
/// [`correlator_header`] is the same value as `http`'s type, for a binding that wants
/// to put it in a [`HeaderMap`](http::HeaderMap).
pub const CORRELATOR_HEADER: &str = "x-nlip-correlator";

/// [`CORRELATOR_HEADER`] as `http`'s own type.
///
/// A function rather than a `const`, because [`HeaderName::from_static`] is not
/// `const`. It cannot fail: the value above is lower-case ASCII, which is the whole of
/// what `from_static` asks for.
pub fn correlator_header() -> HeaderName {
    HeaderName::from_static(CORRELATOR_HEADER)
}

/// The media type this binding writes on a reply.
///
/// JSON, per ECMA-430 §5 ¶2 — "each message exchanged via NLIP is primarily
/// represented as a JSON message" — with ECMA-431 making none of §5 ¶3's special
/// provisions; `application/json`, the media type ECMA-433 §6.1.5 fixes for the same
/// payload ("if the message data is in JSON format, the content-type shall be
/// 'application/json'"). ECMA-431 §6 names neither: it fixes the URL and the method
/// and no more, which is also why a *request*'s own `Content-Type` is not worth
/// enforcing — a peer that sends JSON under `text/plain` has sent a message.
pub const CONTENT_TYPE: &str = "application/json";

/// Largest request body an end-point accepts by default: 8 MiB.
///
/// Large enough for a base64 image in a `binary` submessage (ECMA-430 §5.2.2), small
/// enough that a peer cannot make the end-point hold an arbitrary buffer per
/// connection — ECMA-434 §7.2.5's throttling, applied to size rather than to rate.
///
/// A binding enforces it *while reading the body*, so that exceeding it is answered
/// with [`too_large`](crate::too_large) at [`StatusMap::too_large`](crate::StatusMap)
/// rather than with a framework's plain text.
pub const DEFAULT_MAX_REQUEST_BODY_SIZE: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_correlator_header_is_the_same_name_in_both_spellings() {
        // `HeaderName::from_static` panics on anything but lower-case ASCII, so this
        // is the test that the `&str` above stays a legal header name.
        assert_eq!(correlator_header().as_str(), CORRELATOR_HEADER);
    }
}
