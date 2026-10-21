//! `export interface protocol` — ECMA-430 clause 5, as pure functions.
//!
//! Nothing here has state, a clock or an end-point, and nothing here can trap: peer
//! input reaches `decode` as bytes and comes back as a `result`, and every other
//! function takes a `message` the contract makes total (`format` is a closed enum, and
//! `content.structured` has a defined answer for a string that is not JSON).
//!
//! One seed each — `wit/README.md` §3, the `protocol` block.

use nlipsy_core::{first_submessage_token, token_count};
use nlipsy_protocol::{Message, Wire};

use crate::Component;
use crate::bindings::exports::nlipsy::engine::protocol::Guest;
use crate::bindings::nlipsy::engine::types as wt;
use crate::convert::{
    message_in, message_out, parse_error_out, submessage_in, submessage_out, violation_out, wire_in,
};

impl Guest for Component {
    fn decode(bytes: Vec<u8>, wire: wt::Wire) -> Result<wt::Message, wt::ParseError> {
        let decoded = match wire_in(wire) {
            Wire::Json => Message::from_json_bytes(&bytes),
            Wire::Cbor => Message::from_cbor_slice(&bytes),
        };
        match decoded {
            Ok(message) => Ok(message_out(&message)),
            Err(error) => Err(parse_error_out(&error)),
        }
    }

    fn encode(message: wt::Message, wire: wt::Wire) -> Vec<u8> {
        let message = message_in(message);
        match wire_in(wire) {
            Wire::Json => message.to_json_vec(),
            Wire::Cbor => message.to_cbor_vec(),
        }
    }

    fn validate(message: wt::Message) -> Vec<wt::Violation> {
        message_in(message)
            .validate()
            .iter()
            .map(violation_out)
            .collect()
    }

    fn is_control(message: wt::Message) -> bool {
        message_in(message).is_control()
    }

    fn extract_text(message: wt::Message, language: Option<String>) -> Option<String> {
        message_in(message).extract_text(language.as_deref())
    }

    fn first_submessage_token(message: wt::Message) -> Option<wt::Submessage> {
        first_submessage_token(&message_in(message))
            .as_ref()
            .map(submessage_out)
    }

    fn token_count(message: wt::Message) -> u32 {
        // Saturating, like every other count that crosses: a message with more than
        // four billion token submessages cannot be held in the linear memory that
        // decoded it, so the clamp is unreachable and is here because the cast is not
        // allowed to be the silent one.
        u32::try_from(token_count(&message_in(message))).unwrap_or(u32::MAX)
    }

    /// Written out rather than delegating, because the engine has no three-way
    /// classifier to delegate *to*: the two predicates are `Submessage`'s
    /// (`crates/nlipsy-protocol/src/message.rs:209` and `:204`) and the third case is
    /// their absence. It is the same reading `canonical_tokens` takes in the corpus
    /// runner (`crates/conformance/src/lib.rs:335`), which is what the `component` mode
    /// compares this against.
    fn token_kind_of(submessage: wt::Submessage) -> Option<wt::TokenKind> {
        let submessage = submessage_in(submessage);
        if submessage.is_conversation_token() {
            Some(wt::TokenKind::Conversation)
        } else if submessage.is_authentication_token() {
            Some(wt::TokenKind::Authentication)
        } else {
            // The corpus's `"kind": "other"`: §5.3 reserves exactly two prefixes and
            // §6.2 leaves every other subformat to the application.
            None
        }
    }

    fn upload_request(ask: String) -> wt::Message {
        message_out(&Message::upload_request(ask))
    }

    fn upload_response(uri: String) -> wt::Message {
        message_out(&Message::upload_response(uri))
    }
}
