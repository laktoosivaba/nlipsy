//! The three messages this binding builds itself.
//!
//! One is a request — TR/113 §11.1's chat turn, which is what a client agent sends
//! when it has nothing more complicated to say. Two are replies the *binding* produces
//! before clause 6 is ever reached: a method that is not `POST`, and a body larger than
//! the end-point will read. Neither of those two can echo anything — there was no
//! message to take a token from — and both are still NLIP messages, because the
//! principle the reference server stated and this workspace inherits is that *a request
//! that cannot be understood is answered with a non-2xx status **and** an NLIP
//! `MessageType: Error` body, so a peer reading only the payload and one reading only
//! the status line both learn what happened.*
//!
//! They are functions rather than constants because a message is an owned value, and
//! they are *here* rather than in either binding because a fetch handler written in
//! another language has to produce the same bodies.

use nlipsy_protocol::message::{Format, Message, Submessage};

use crate::binding::NLIP_PATH;

/// Subformat prefix for the conversation token, ECMA-430 §6.2.1.
pub const CONVERSATION_SUBFORMAT: &str = "conversation";

/// Build the message a chat turn sends.
///
/// The conversation token is the client's own and is sent from the first turn. §6.2.1
/// puts either peer first — "Either the client agent or the server agent may be the
/// first one to create a submessage with format field of 'token' and the subformat
/// field starting with 'conversation'" — and a client that brings its own identifier
/// never has to be handed one, never loses it to a restart, and never has to trust a
/// value that arrived over the network to be a usable name for a local file.
///
/// `messagetype` is not set. TR/113 §11.1 gives the canonical chat exchange without
/// one, and the end-point has no use for it here.
///
/// It is two lines — `Message::new(…).with_submessage(Submessage::token(…))` — and
/// where those two lines live was left open by `runtime-plan.md` §8, because they take
/// a reading of §6.2.1 on the caller's behalf by putting the conversation's identifier
/// in a submessage of its own. **It is settled here.** `nlipsy-core` still decides
/// nothing about what a message means, `nlipsy-protocol` still models messages rather
/// than exchanges, and this crate is the one that holds ECMA-431 §6 and TR/113 §11.1 as
/// values for both ends of the binding — which is exactly what a constructor for the
/// canonical chat turn is. `nlipsy-http-client` re-exports it, so the path a caller
/// already wrote keeps working.
pub fn request(text: &str, language: &str, conversation: &str) -> Message {
    Message::new(Format::Text, language, text.to_string())
        .with_submessage(Submessage::token(CONVERSATION_SUBFORMAT, conversation))
}

/// The body for a method other than `POST` on the end-point's own path.
///
/// ECMA-431 §6 obliges `POST` and forbids nothing, so the *status* is the binding's
/// choice like every other — `405` here, with whatever `Allow` header the framework
/// knows to attach. The *body* is the crate's principle applied to the one method
/// mismatch an end-point controls: a status line alone tells a peer reading the payload
/// nothing. A mismatch on some *other* path is a fallback, and a fallback belongs to
/// the deployment.
pub fn method_not_allowed() -> Message {
    Message::error(format!(
        "NLIP messages are accepted with POST at {NLIP_PATH} (ECMA-431 §6)"
    ))
}

/// The body for a request larger than `limit` bytes.
///
/// §6.2 cannot be honoured on this one — the body was never decoded, so no token was
/// ever read — and ECMA-431 §6.1 still is: what goes back is an NLIP message with a
/// `text` first submessage. It is also the one refusal after which a peer should treat
/// its state as untouched: nothing it sent was consumed.
///
/// The limit is named in the text on purpose. A peer that is told the number can split
/// its payload or use ECMA-430 §6.4's out-of-band leg; one that is told only "too
/// large" can do neither.
pub fn too_large(limit: usize) -> Message {
    Message::error(format!(
        "the request body exceeds this endpoint's limit of {limit} bytes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_request_matches_the_canonical_exchange_from_tr113() {
        let message = request("What is Ecma?", "english", "c1");
        let value: serde_json::Value = serde_json::from_str(&message.to_json_string()).unwrap();

        assert_eq!(
            value,
            json!({
                "format": "text",
                "subformat": "english",
                "content": "What is Ecma?",
                "submessages": [{
                    "format": "token",
                    "subformat": "conversation",
                    "content": "c1"
                }]
            })
        );
    }

    #[test]
    fn both_pre_clause_six_bodies_are_error_messages_a_peer_can_read() {
        for message in [method_not_allowed(), too_large(8 * 1024 * 1024)] {
            // §5.1.1's one value for the field, which is the only in-band marker a
            // peer has for a failure.
            assert_eq!(message.message_type.as_deref(), Some("Error"));
            assert_eq!(message.format, Format::Text);
            assert!(message.content_as_str().is_some());
            // Nothing is echoed: neither reply had a decoded request to echo from.
            assert!(message.submessages.is_empty());
        }

        assert!(
            method_not_allowed()
                .content_as_str()
                .is_some_and(|text| text.contains(NLIP_PATH)),
            "a peer that used the wrong method is told which path and which one works"
        );
        assert!(
            too_large(4096)
                .content_as_str()
                .is_some_and(|text| text.contains("4096")),
            "the number is what lets a peer split the payload instead of guessing"
        );
    }
}
