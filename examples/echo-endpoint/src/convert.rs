//! The contract's message and the model's, in both directions.
//!
//! An end-point component sees clause 5's vocabulary and nothing else: what a request
//! *is*, and what a reply is. Clause 6 — the echo, the ceiling, the minting, the order —
//! is the engine's, on the other side of `wac plug`, and this crate has no idea any of
//! it is happening.
//!
//! It is the third copy of these conversions in the tree, after
//! `crates/nlipsy-component/src/convert.rs` and `crates/conformance/src/component.rs`,
//! and that is what a contract is for: each side writes its own and they meet at the
//! WIT. A deployment that reached into the engine's crate for them would be linking the
//! engine rather than composing with it.

use nlipsy_protocol::{Content, Format, Message, Submessage};

use crate::bindings::nlipsy::engine::types::{
    Content as WitContent, Format as WitFormat, Message as WitMessage, Number as WitNumber,
    Submessage as WitSubmessage,
};

/// Table 1's six values, contract to model.
fn format_in(format: WitFormat) -> Format {
    match format {
        WitFormat::Text => Format::Text,
        WitFormat::Token => Format::Token,
        WitFormat::Structured => Format::Structured,
        WitFormat::Binary => Format::Binary,
        WitFormat::Location => Format::Location,
        WitFormat::Generic => Format::Generic,
    }
}

/// Table 1's six values, model to contract.
fn format_out(format: Format) -> WitFormat {
    match format {
        Format::Text => WitFormat::Text,
        Format::Token => WitFormat::Token,
        Format::Structured => WitFormat::Structured,
        Format::Binary => WitFormat::Binary,
        Format::Location => WitFormat::Location,
        Format::Generic => WitFormat::Generic,
    }
}

/// One content value, contract to model.
///
/// `structured` is the canonical JSON text of an array or an object, because WIT has no
/// recursive types. This end-point never looks inside one — it echoes text — so it is
/// carried as `text` content rather than parsed, which is the boundary's own defined
/// answer for a string it cannot read as JSON and costs no `serde_json` here.
fn content_in(content: WitContent) -> Content {
    match content {
        WitContent::Null => Content::Null,
        WitContent::Bool(value) => Content::Bool(value),
        WitContent::Number(WitNumber::Int(value)) => Content::Number(value.into()),
        WitContent::Number(WitNumber::Uint(value)) => Content::Number(value.into()),
        WitContent::Number(WitNumber::Float(value)) => match serde_json::Number::from_f64(value) {
            Some(number) => Content::Number(number),
            None => Content::Null,
        },
        WitContent::Text(text) | WitContent::Structured(text) => Content::String(text),
        WitContent::Bytes(bytes) => Content::Bytes(bytes),
    }
}

/// One content value, model to contract.
fn content_out(content: &Content) -> WitContent {
    match content {
        Content::Null => WitContent::Null,
        Content::Bool(value) => WitContent::Bool(*value),
        Content::Number(number) => {
            if let Some(value) = number.as_i64() {
                WitContent::Number(WitNumber::Int(value))
            } else if let Some(value) = number.as_u64() {
                WitContent::Number(WitNumber::Uint(value))
            } else {
                WitContent::Number(WitNumber::Float(number.as_f64().unwrap_or_default()))
            }
        }
        Content::String(text) => WitContent::Text(text.clone()),
        Content::Bytes(bytes) => WitContent::Bytes(bytes.clone()),
        Content::Array(_) | Content::Object(_) => {
            WitContent::Structured(content.to_json_value().to_string())
        }
    }
}

/// One submessage, contract to model.
fn submessage_in(submessage: WitSubmessage) -> Submessage {
    let mut out = Submessage::new(
        format_in(submessage.format),
        submessage.subformat,
        content_in(submessage.content),
    );
    out.label = submessage.label;
    out
}

/// One submessage, model to contract.
fn submessage_out(submessage: &Submessage) -> WitSubmessage {
    WitSubmessage {
        label: submessage.label.clone(),
        format: format_out(submessage.format),
        subformat: submessage.subformat.clone(),
        content: content_out(&submessage.content),
    }
}

/// One message, contract to model.
pub fn message_in(message: WitMessage) -> Message {
    let mut out = Message::new(
        format_in(message.format),
        message.subformat,
        content_in(message.content),
    );
    out.message_type = message.message_type;
    out.control = message.control;
    out.label = message.label;
    out.submessages = message.submessages.into_iter().map(submessage_in).collect();
    out
}

/// One message, model to contract.
pub fn message_out(message: &Message) -> WitMessage {
    WitMessage {
        message_type: message.message_type.clone(),
        control: message.control,
        label: message.label.clone(),
        format: format_out(message.format),
        subformat: message.subformat.clone(),
        content: content_out(&message.content),
        submessages: message.submessages.iter().map(submessage_out).collect(),
    }
}
