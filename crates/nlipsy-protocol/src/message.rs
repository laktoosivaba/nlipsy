//! NLIP message model — ECMA-430 clause 5.
//!
//! A message is a *first submessage* (`Format`, `Subformat`, `Content` required;
//! `MessageType` optional) plus an ordered list of additional submessages.
//!
//! Three rules from the standard drive most of the code here:
//!
//! * §5 — "Capitalization is not relevant to the name or value of the field."
//!   Decoding therefore matches field names case-insensitively and parses format
//!   values the same way. Encoding emits the canonical lower-case form, which is
//!   what the TR/113 examples use — and what the reference implementation
//!   (`nlip_sdk`) does too, though that is corroboration rather than authority.
//! * §5.1.5 — "the order of the submessages field is significant", so the
//!   submessages are kept in a `Vec` and never reordered.
//! * ECMA-432 §7.1 and §7.2 — the same message is CBOR on a binary WebSocket frame
//!   and JSON on a text one, and binary content is a byte string in the first and
//!   base64 in the second. That difference, and nothing else, is what [`Wire`]
//!   names; [`Message::from_content`] is the single decoder both spellings go
//!   through, so the base64 rule exists in exactly one place.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::de;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::content::{Content, Wire, base64_decode};
use crate::error::{ParseError, Violation};

/// Values allowed in the `Format` field — ECMA-430 Table 1.
///
/// Exhaustive on purpose, in contrast to [`Message`] and [`Submessage`]: Table 1 is a
/// closed list in the standard, and "the format field must have one of the values
/// described in Table 1" (§5.3). A seventh value would be a revision of ECMA-430, so a
/// consumer's `match` is allowed to be exhaustive and to keep being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Natural language text; the subformat names the language (`<lang>`).
    Text,
    /// An opaque token interpreted by end-points; the subformat is
    /// `<prefix>_<suffix>`, with `authentication` and `conversation` reserved (§6.2).
    Token,
    /// Structured content: `json`, `uri`, `xml`, `html`, or the name of a programming
    /// language.
    Structured,
    /// Binary content; the subformat is `<content>/<encoding>`, e.g. `image/jpeg`.
    Binary,
    /// A location, as a text description or a GPS reading.
    Location,
    /// A private extension agreed between the two end-points; the subformat names it.
    Generic,
}

impl Format {
    /// Every value of Table 1, in the order the table lists them.
    pub const ALL: [Format; 6] = [
        Format::Text,
        Format::Token,
        Format::Structured,
        Format::Binary,
        Format::Location,
        Format::Generic,
    ];

    /// The values of Table 1 as prose, for an error message that has to list them.
    const EXPECTED: &'static str =
        "text, token, structured, binary, location or generic (ECMA-430 Table 1)";

    /// Canonical (lower-case) wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Format::Text => "text",
            Format::Token => "token",
            Format::Structured => "structured",
            Format::Binary => "binary",
            Format::Location => "location",
            Format::Generic => "generic",
        }
    }

    /// Parse a `Format` value. Case-insensitive, as §5 requires.
    pub fn parse(value: &str) -> Option<Format> {
        Format::ALL
            .into_iter()
            .find(|format| value.eq_ignore_ascii_case(format.as_str()))
    }
}

impl FromStr for Format {
    /// A failure is the same [`ParseError::UnknownFormat`] a malformed message
    /// produces, rather than a type of its own: the two say the same thing, and a
    /// caller that already handles `ParseError` should not have to learn a second error
    /// for the same mistake. The path is `$`, because a bare string has no position in
    /// a document.
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Format, ParseError> {
        Format::parse(value).ok_or_else(|| ParseError::UnknownFormat {
            path: "$".into(),
            value: value.to_owned(),
            expected: Format::EXPECTED,
        })
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Format {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The canonical lower-case spelling, on every wire: §5 makes the value's case
        // irrelevant on the way in, which says nothing about what to emit, and the
        // standard's own examples are lower case.
        serializer.serialize_str(self.as_str())
    }
}

/// A submessage — ECMA-430 §5.2.
///
/// `#[non_exhaustive]`: the standard's field list has grown before (ECMA-432 §10 is
/// already contemplating a session identifier "in `MessageType` or custom field"), and
/// a binding may need to carry something this crate does not model yet. Construct one
/// through [`Submessage::new`] and the builders, and match with `..`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Submessage {
    /// Free-form annotation, e.g. a sequence number or a chat role (§5.2.1).
    pub label: Option<String>,
    /// How `content` is represented (§5.2.2).
    pub format: Format,
    /// The subcategory within `format` (§5.2.3).
    pub subformat: String,
    /// The payload itself (§5.2.4).
    pub content: Content,
}

impl Submessage {
    /// Build a submessage from the three fields §5.2 requires.
    pub fn new(format: Format, subformat: impl Into<String>, content: impl Into<Content>) -> Self {
        Submessage {
            label: None,
            format,
            subformat: subformat.into(),
            content: content.into(),
        }
    }

    /// Attach a `Label` (§5.2.1).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A `token` submessage, the only kind the protocol assigns meaning to (§6.2).
    pub fn token(subformat: impl Into<String>, content: impl Into<String>) -> Self {
        Submessage::new(Format::Token, subformat, Content::String(content.into()))
    }

    /// A `text/english` submessage, mirroring [`Message::text`] — the one pairing every
    /// conforming end-point must support (§5.3).
    pub fn text(content: impl Into<String>) -> Self {
        Submessage::new(Format::Text, "english", Content::String(content.into()))
    }

    /// A `binary` submessage carrying raw bytes.
    ///
    /// The bytes stay bytes: they become a CBOR byte string on a binary wire (ECMA-432
    /// §7.1) and base64 in JSON (§6.4), and the caller does not choose between those.
    pub fn binary(subformat: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        Submessage::new(Format::Binary, subformat, Content::Bytes(content.into()))
    }

    /// A `structured/uri` submessage — the shape §6.4 ¶3 describes.
    ///
    /// §6.4 ¶3 asks the response to a large-upload request to be "a message with
    /// MessageType field set to 'control' containing a submessage with the format
    /// field set of 'structured' and subformat of a URI", so the URI belongs in a
    /// *submessage* of a control message. This builds that submessage;
    /// [`Message::uri`] builds the same pair as a first submessage instead.
    pub fn uri(uri: impl Into<String>) -> Self {
        Submessage::new(Format::Structured, "uri", Content::String(uri.into()))
    }

    /// `true` for submessages with `Format: token`.
    pub fn is_token(&self) -> bool {
        self.format == Format::Token
    }

    /// `true` for `token` submessages whose subformat starts with `authentication` (§6.2.2).
    ///
    /// §5.3 reserves two prefixes and no more — `authentication` and `conversation` —
    /// and §6.2 says of every other token subformat that it "may also contain a
    /// subformat which can be any general string … The use of these tokens is up to the
    /// application". `authorization`, which the reference Python SDK emits in place of
    /// `authentication`, is therefore application-defined: echoed like any other token,
    /// because §6.2 does not condition the echo on the prefix, but not treated as an
    /// authentication credential — nothing counts the peer as having brought one, and
    /// nothing carries it forward as one.
    pub fn is_authentication_token(&self) -> bool {
        self.is_token() && starts_with_ignore_case(&self.subformat, &["authentication"])
    }

    /// `true` for `token` submessages whose subformat starts with `conversation` (§6.2.1).
    pub fn is_conversation_token(&self) -> bool {
        self.is_token() && starts_with_ignore_case(&self.subformat, &["conversation"])
    }

    /// The content rendered as a string, if it is one.
    pub fn content_as_str(&self) -> Option<&str> {
        self.content.as_str()
    }

    /// The content as bytes, if it is a byte string.
    ///
    /// A `binary` submessage decoded from either wire answers here rather than in
    /// [`content_as_str`](Submessage::content_as_str): the base64 of a JSON frame is
    /// undone at decode time, so both wires produce the same value.
    pub fn content_as_bytes(&self) -> Option<&[u8]> {
        self.content.as_bytes()
    }

    /// Decode one submessage from a [`Content`] value that arrived over `wire`.
    ///
    /// Errors name `$`, because a submessage decoded on its own is its own document.
    /// Inside a message, [`Message::from_content`] supplies `submessages[i]` instead.
    pub fn from_content(content: Content, wire: Wire) -> Result<Submessage, ParseError> {
        Submessage::decode(content, wire, "$")
    }

    /// Semantic checks for one submessage, appended to `findings`.
    fn validate_into(&self, path: &str, findings: &mut Vec<Violation>) {
        validate_subformat(self.format, &self.subformat, path, findings);
        validate_content(self.format, &self.content, path, findings);
    }

    fn decode(content: Content, wire: Wire, path: &str) -> Result<Submessage, ParseError> {
        let Content::Object(mut fields) = content else {
            return Err(ParseError::NotObject { path: path.into() });
        };

        let label = take_string(&mut fields, "label", path)?;
        let format = take_format(&mut fields, "format", path)?;
        let subformat = take_required_string(&mut fields, "subformat", path)?;
        let content = take_content(&mut fields, format, wire, path)?;

        Ok(Submessage {
            label,
            format,
            subformat,
            content,
        })
    }
}

impl Serialize for Submessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3 + usize::from(self.label.is_some())))?;
        if let Some(label) = &self.label {
            map.serialize_entry("label", label)?;
        }
        map.serialize_entry("format", &self.format)?;
        map.serialize_entry("subformat", &self.subformat)?;
        map.serialize_entry("content", &self.content)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Submessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Submessage, D::Error> {
        let wire = wire_of(&deserializer);
        let content = Content::deserialize(deserializer)?;
        Submessage::from_content(content, wire).map_err(de::Error::custom)
    }
}

/// An NLIP message: the first submessage plus an ordered list of further ones.
///
/// `#[non_exhaustive]` for the same reason as [`Submessage`]: fields will be added.
/// Build one with [`Message::new`] or one of the shorthands, adjust the public fields
/// afterwards, and match with `..`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Message {
    /// Optional classification. The value `control` is reserved (§5.1.1); TR/113 §10
    /// recommends other self-describing values such as `Error` or `Debugging`.
    pub message_type: Option<String>,
    /// Legacy spelling of the control flag used by TR/113 examples and accepted by §6.3.
    pub control: Option<bool>,
    /// Not defined by ECMA-430 for the first submessage, but emitted by the reference
    /// SDK, so it is carried through rather than dropped.
    pub label: Option<String>,
    /// How `content` is represented (§5.1.2).
    pub format: Format,
    /// The subcategory within `format` (§5.1.3).
    pub subformat: String,
    /// The payload itself (§5.1.4).
    pub content: Content,
    /// Order is significant (§5.1.5).
    pub submessages: Vec<Submessage>,
}

impl Message {
    /// Build a message from its first submessage.
    pub fn new(format: Format, subformat: impl Into<String>, content: impl Into<Content>) -> Self {
        Message {
            message_type: None,
            control: None,
            label: None,
            format,
            subformat: subformat.into(),
            content: content.into(),
            submessages: Vec::new(),
        }
    }

    /// `text/english` — the one pairing every conforming endpoint must support (§5.3).
    pub fn text(content: impl Into<String>) -> Self {
        Message::new(Format::Text, "english", Content::String(content.into()))
    }

    /// A `binary` message carrying raw bytes, e.g. the image of ECMA-432 §8 Example 2.
    ///
    /// The bytes stay bytes until something encodes them: a CBOR byte string on a
    /// binary wire (ECMA-432 §7.1), base64 in JSON (§6.4).
    pub fn binary(subformat: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        Message::new(Format::Binary, subformat, Content::Bytes(content.into()))
    }

    /// A `text/english` message marked `MessageType: Error`, the convention TR/113 §10
    /// recommends for error indications.
    pub fn error(content: impl Into<String>) -> Self {
        let mut message = Message::text(content);
        message.message_type = Some("Error".to_string());
        message
    }

    /// A `structured/uri` message carrying a URI as its first submessage.
    ///
    /// This builds the `structured` / `uri` pair and nothing else, and it is **not**
    /// the §6.4 ¶3 answer: that clause asks for "a message with MessageType field set
    /// to 'control' containing a submessage with the format field set of 'structured'
    /// and subformat of a URI", which is [`Message::upload_response`]. The constructor
    /// stays general because a URI is a Table 1 `structured` subformat like any other,
    /// and §6.4 is only its most common use.
    pub fn uri(uri: impl Into<String>) -> Self {
        Message::new(Format::Structured, "uri", Content::String(uri.into()))
    }

    /// ECMA-430 §6.4 ¶2 — ask a peer for an end-point that takes large uploads.
    ///
    /// The clause fixes the shape: "This must be a message with the MessageType field
    /// set to 'control', and the first submessage should include a text format request
    /// for the end-point to send large uploads." So `ask` becomes the `text` /
    /// `english` first submessage of a `control` message.
    ///
    /// The boolean `control` field is deliberately left unset. §5.1.1's reserved
    /// `MessageType` value and §6.3's "control field set to true" are two spellings of
    /// one thing — [`Message::is_control`] answers for either, and this crate
    /// preserves whichever arrived rather than inventing a second one. §6.4 names
    /// `MessageType` and only that, so that is the field these constructors write; an
    /// end-point answering a request that used the boolean spelling adds the boolean
    /// back under §6.3, which is a separate obligation with its own code.
    pub fn upload_request(ask: impl Into<String>) -> Self {
        Message::text(ask).message_type("control")
    }

    /// ECMA-430 §6.4 ¶3 — answer a [`Message::upload_request`] with the address.
    ///
    /// "The response to this request message from the server agent should be a message
    /// with MessageType field set to 'control' containing a submessage with the format
    /// field set of 'structured' and subformat of a URI." Literally that: `control` as
    /// the `MessageType`, a `text` / `english` first submessage saying what the address
    /// is for, and the URI in a [`Submessage::uri`] — a *submessage*, which is the half
    /// [`Message::uri`] alone cannot supply, since the first submessage is where the
    /// message's own content goes.
    ///
    /// The boolean `control` field is left unset, for the reason given on
    /// [`Message::upload_request`].
    pub fn upload_response(uri: impl Into<String>) -> Self {
        Message::text("Send large binary content to the end-point named in the submessage below.")
            .message_type("control")
            .with_submessage(Submessage::uri(uri))
    }

    /// Set the `MessageType` field (§5.1.1).
    pub fn message_type(mut self, message_type: impl Into<String>) -> Self {
        self.message_type = Some(message_type.into());
        self
    }

    /// Append a submessage, keeping the order §5.1.5 makes significant.
    pub fn with_submessage(mut self, submessage: Submessage) -> Self {
        self.submessages.push(submessage);
        self
    }

    /// `true` when the message asks for a control exchange.
    ///
    /// ECMA-430 is inconsistent here: §5.1.1 reserves the *value* `control` of
    /// `MessageType`, while §6.3 speaks of "the control field set to true" as a
    /// boolean — and TR/113's sample exchanges use `"control": false`. Both
    /// spellings are accepted, and §6.3 is honoured for either.
    pub fn is_control(&self) -> bool {
        self.control.unwrap_or(false)
            || self
                .message_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("control"))
    }

    /// `true` when the *first* submessage has `Format: token` (§5.3 Table 1).
    ///
    /// §5 ¶2 calls the leading object "the first submessage", so a message like this
    /// one is a message containing a token submessage for §6.2's purposes — and the
    /// reference SDK's `create_token` builds exactly it. [`Message::tokens`] cannot
    /// yield it, because the first submessage is not a [`Submessage`] value, which is
    /// why the predicate exists.
    pub fn is_token(&self) -> bool {
        self.format == Format::Token
    }

    /// `true` when the first submessage is a `token` whose subformat starts with
    /// `authentication` (§6.2.2) — see [`Submessage::is_authentication_token`], which
    /// this mirrors for the `submessages` array.
    pub fn is_authentication_token(&self) -> bool {
        self.is_token() && starts_with_ignore_case(&self.subformat, &["authentication"])
    }

    /// `true` when the first submessage is a `token` whose subformat starts with
    /// `conversation` (§6.2.1) — see [`Submessage::is_conversation_token`].
    pub fn is_conversation_token(&self) -> bool {
        self.is_token() && starts_with_ignore_case(&self.subformat, &["conversation"])
    }

    /// Every token submessage in the `submessages` array, in order.
    ///
    /// The first submessage is **not** among them, and cannot be: it is not a
    /// [`Submessage`] value. A message whose first submessage is a token answers
    /// [`Message::is_token`] instead, so an endpoint honouring §6.2 ¶2 — "include the
    /// same submessage with the exact same format, subformat and content in its
    /// response" — must consider both this iterator and that predicate. The clause's
    /// own exception, tokens this endpoint created, needs the endpoint's memory of
    /// what it issued and is the caller's to apply (`tokens().filter(..)`).
    pub fn tokens(&self) -> impl Iterator<Item = &Submessage> {
        self.submessages.iter().filter(|s| s.is_token())
    }

    /// The content rendered as a string, if it is one.
    pub fn content_as_str(&self) -> Option<&str> {
        self.content.as_str()
    }

    /// The content as bytes, if it is a byte string — see
    /// [`Submessage::content_as_bytes`].
    pub fn content_as_bytes(&self) -> Option<&[u8]> {
        self.content.as_bytes()
    }

    /// Decode a message from JSON bytes.
    ///
    /// Equivalent to parsing into a [`Content`] and calling [`Message::from_content`]
    /// with [`Wire::Json`], so a `binary` message's base64 is decoded into
    /// [`Content::Bytes`] here as everywhere else.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Message, ParseError> {
        let content: Content = serde_json::from_slice(bytes)?;
        Message::from_content(content, Wire::Json)
    }

    /// Decode a message from a JSON string.
    pub fn from_json_str(json: &str) -> Result<Message, ParseError> {
        let content: Content = serde_json::from_str(json)?;
        Message::from_content(content, Wire::Json)
    }

    /// Decode a message from an already-parsed JSON value.
    pub fn from_value(value: Value) -> Result<Message, ParseError> {
        Message::from_content(Content::from(value), Wire::Json)
    }

    /// Decode a message from a [`Content`] value that arrived over `wire`.
    ///
    /// This is the decoder. Every other entry point — [`Message::from_json_bytes`],
    /// [`Message::from_value`], the CBOR pair, the [`Deserialize`] impl — turns its
    /// input into a `Content` and
    /// calls this, so clause 5 is read in one place and the [`Wire`] is the only thing
    /// that differs between them.
    ///
    /// What it enforces:
    ///
    /// * field names are matched case-insensitively (§5), and a field present twice
    ///   under different casings is [`ParseError::DuplicateField`] rather than resolved
    ///   — the standard makes the names equivalent and does not say which wins;
    /// * `format`, `subformat` and `content` are required (§5.1.2–5.1.4, §5.2.2–5.2.4);
    /// * `format` must be a value of Table 1 (§5.3);
    /// * `submessages`, when present and not null, must be an array (§5.1.5), and an
    ///   error inside one is reported as `submessages[i]`;
    /// * when `wire` is [`Wire::Json`] and `format` is `binary`, a string `content` is
    ///   base64 and is decoded into [`Content::Bytes`] (§6.4, ECMA-432 §7.2). Over
    ///   [`Wire::Cbor`] it is left alone: §7.1 puts raw bytes in a byte string, so a
    ///   text string there is text.
    ///
    /// Decoding base64 here, rather than lazily at the accessor, is what makes
    /// transcoding work: a message read from a JSON frame and written to a CBOR one
    /// must come out as a byte string (ECMA-432 §7.1), and it can only do that if the
    /// model is holding bytes rather than a string that is *about* bytes.
    ///
    /// Fields the standard does not define are **dropped**, not preserved: §5 lists the
    /// fields a message has, an unknown one has no defined meaning to carry forward,
    /// and keeping it would mean re-emitting something this crate cannot say is
    /// correct. A peer that needs to round-trip an extension should keep the bytes it
    /// received (see also §5.1.1's "its interpretation is left to the end-point").
    pub fn from_content(content: Content, wire: Wire) -> Result<Message, ParseError> {
        let Content::Object(mut fields) = content else {
            return Err(ParseError::NotObject { path: "$".into() });
        };

        let message_type = take_string(&mut fields, "messagetype", "$")?;
        let control = take_bool(&mut fields, "control", "$")?;
        let label = take_string(&mut fields, "label", "$")?;
        let format = take_format(&mut fields, "format", "$")?;
        let subformat = take_required_string(&mut fields, "subformat", "$")?;
        let content = take_content(&mut fields, format, wire, "$")?;

        let submessages = match take(&mut fields, "submessages", "$")? {
            // Absent and null are both "no submessages": §5.1.5 makes the field
            // optional, and an empty list is what its absence means. (`content` is
            // different — it is required, so a null there is a value, not an absence.)
            None | Some(Content::Null) => Vec::new(),
            Some(Content::Array(items)) => items
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    Submessage::decode(item, wire, &format!("submessages[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(other) => {
                return Err(ParseError::WrongType {
                    path: "$".into(),
                    field: "submessages".into(),
                    expected: "an array",
                    found: other.type_name(),
                });
            }
        };

        Ok(Message {
            message_type,
            control,
            label,
            format,
            subformat,
            content,
            submessages,
        })
    }

    /// Decode a message from a CBOR document — ECMA-432 §7.1.
    ///
    /// The frame must hold *one* message and nothing else: §7.1 says a binary message
    /// "shall contain a single NLIP message encoded in CBOR", and a CBOR reader stops
    /// at the end of the first item, so two concatenated frames would otherwise decode
    /// as the first with the second silently dropped. Anything left over is
    /// [`ParseError::TrailingBytes`] at `$`. The JSON wire already answers this way
    /// without help — `serde_json` refuses trailing characters — and the two wires
    /// agreeing is the point.
    ///
    /// The example of ECMA-432 §8 Example 2 — an image processing request — round-trips
    /// through a binary frame. The doctest exists only when the feature does, because
    /// the item it documents does.
    ///
    /// ```
    /// use nlipsy_protocol::{Message, Submessage};
    ///
    /// let request = Message::binary("image/jpeg", vec![0xff, 0xd8, 0xff, 0xe0])
    ///     .with_submessage(Submessage::text("Process this image for defects"));
    ///
    /// let frame = request.to_cbor_vec();
    /// // The payload travels as a CBOR byte string, not as base64 (§7.1).
    /// assert!(frame.windows(4).any(|window| window == [0xff, 0xd8, 0xff, 0xe0]));
    /// assert_eq!(Message::from_cbor_slice(&frame).unwrap(), request);
    /// ```
    #[cfg(feature = "cbor")]
    #[cfg_attr(docsrs, doc(cfg(feature = "cbor")))]
    pub fn from_cbor_slice(bytes: &[u8]) -> Result<Message, ParseError> {
        // `&mut &[u8]` rather than `&[u8]`: reading through the mutable reference
        // advances the slice, which is the only way to see what the decoder did not
        // consume. Reading the remainder is not the same as decoding it — a second
        // item may be malformed, and §7.1 is broken either way.
        let mut rest = bytes;
        let content: Content = ciborium::de::from_reader(&mut rest)?;
        if !rest.is_empty() {
            return Err(ParseError::TrailingBytes {
                remaining: rest.len(),
            });
        }
        Message::from_content(content, Wire::Cbor)
    }

    /// Encode as a CBOR document — ECMA-432 §7.1.
    ///
    /// [`Content::Bytes`] becomes a CBOR byte string (RFC 8949 major type 2), which is
    /// what §8 Examples 1 and 2 send and what §7.1 means by "Byte string (raw binary)".
    #[cfg(feature = "cbor")]
    #[cfg_attr(docsrs, doc(cfg(feature = "cbor")))]
    pub fn to_cbor_vec(&self) -> Vec<u8> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out).expect(CANNOT_FAIL);
        out
    }

    /// Encode as JSON, in the canonical lower-case form.
    ///
    /// [`Content::Bytes`] becomes base64 here, because that is JSON's only spelling for
    /// it (§6.4).
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect(CANNOT_FAIL)
    }

    /// Encode as a JSON string.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).expect(CANNOT_FAIL)
    }

    /// Encode as JSON bytes — what an ECMA-431 body or an ECMA-432 §7.2 text frame
    /// carries.
    pub fn to_json_vec(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect(CANNOT_FAIL)
    }

    /// Semantic conformance findings for this message.
    ///
    /// A [`Violation`] means the message was understood but broke a rule of clause 5;
    /// a deployment decides whether to reject it or merely log it. Contrast
    /// [`ParseError`], which means there was nothing to understand.
    pub fn validate(&self) -> Vec<Violation> {
        let mut findings = Vec::new();
        validate_subformat(self.format, &self.subformat, "$", &mut findings);
        validate_content(self.format, &self.content, "$", &mut findings);
        for (index, submessage) in self.submessages.iter().enumerate() {
            submessage.validate_into(&format!("submessages[{index}]"), &mut findings);
        }
        findings
    }

    /// All `text` content of the message in `language`, joined in document order.
    ///
    /// The first submessage comes first, then the `Submessages` array in the order it
    /// arrived — which §5.1.5 makes significant. A `language` of `None` collects every
    /// text subformat, mirroring the reference SDK's `extract_text`.
    pub fn extract_text(&self, language: Option<&str>) -> Option<String> {
        let wanted = |subformat: &str| {
            language.is_none_or(|language| subformat.eq_ignore_ascii_case(language))
        };

        // Written as nested `if let` rather than as a let-chain: a let-chain in an `&&`
        // sequence needs Rust 1.88, and the crate's stated MSRV is 1.85 — the edition
        // 2024 floor. Nothing else here needs more than that.
        let mut parts: Vec<&str> = Vec::new();
        if self.format == Format::Text && wanted(&self.subformat) {
            if let Some(text) = self.content.as_str() {
                parts.push(text);
            }
        }
        for submessage in &self.submessages {
            if submessage.format == Format::Text && wanted(&submessage.subformat) {
                if let Some(text) = submessage.content.as_str() {
                    parts.push(text);
                }
            }
        }

        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Why encoding a [`Message`] has no failure mode, spelled once for the three
/// `expect`s that need to say it.
///
/// A serializer fails on data it cannot represent: a map with non-string keys, or a
/// non-finite float. A `Message` has neither. Its own keys are literals; a
/// [`Content::Object`] is keyed by `String`; and a [`Content::Number`] is a
/// `serde_json::Number`, which `Number::from_f64` has already refused to build out of
/// a NaN or an infinity. The alternative to `expect` here is not a safer API but a
/// `Result` every caller would unwrap — or, as this crate used to do, a silent
/// fallback that emitted an empty text message in place of the real one, which is
/// wrong data standing where an error should have been.
const CANNOT_FAIL: &str =
    "a Message always encodes: its keys are strings and its numbers are finite";

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let length = 3
            + usize::from(self.message_type.is_some())
            + usize::from(self.control.is_some())
            + usize::from(self.label.is_some())
            + usize::from(!self.submessages.is_empty());

        // Canonical lower-case keys, in the order clause 5 introduces the fields.
        let mut map = serializer.serialize_map(Some(length))?;
        if let Some(message_type) = &self.message_type {
            map.serialize_entry("messagetype", message_type)?;
        }
        if let Some(control) = self.control {
            map.serialize_entry("control", &control)?;
        }
        if let Some(label) = &self.label {
            map.serialize_entry("label", label)?;
        }
        map.serialize_entry("format", &self.format)?;
        map.serialize_entry("subformat", &self.subformat)?;
        map.serialize_entry("content", &self.content)?;
        // Omitted when empty: §5.1.5 makes the field optional, and an empty array says
        // nothing an absent field does not.
        if !self.submessages.is_empty() {
            map.serialize_entry("submessages", &self.submessages)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Message {
    /// Decodes through [`Message::from_content`], so serde and the direct entry points
    /// cannot drift apart.
    ///
    /// The typed [`ParseError`] is lost on this path — serde's error type is the
    /// deserializer's, and all it can carry is a string — so a caller that wants to
    /// know *which* rule was broken, and where, should call [`Message::from_json_bytes`]
    /// or [`Message::from_cbor_slice`] instead. The message text is the same either way.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Message, D::Error> {
        let wire = wire_of(&deserializer);
        let content = Content::deserialize(deserializer)?;
        Message::from_content(content, wire).map_err(de::Error::custom)
    }
}

/// Which wire a deserializer is reading.
///
/// `is_human_readable` is serde's own name for this distinction, and it splits exactly
/// where ECMA-432 does: `serde_json` answers `true` (§7.2, base64) and `ciborium`
/// `false` (§7.1, byte strings).
fn wire_of<'de, D: Deserializer<'de>>(deserializer: &D) -> Wire {
    if deserializer.is_human_readable() {
        Wire::Json
    } else {
        Wire::Cbor
    }
}

// ---------------------------------------------------------------------------
// Decoding helpers
//
// Field access is by case-insensitive match over the decoded object. That keeps
// the rule from §5 in one place, and lets a lookup notice when a document carries
// the same field twice under different casings — an ambiguity the standard does
// not resolve and that silently picking one would hide.
// ---------------------------------------------------------------------------

type Fields = BTreeMap<String, Content>;

fn take(fields: &mut Fields, field: &str, path: &str) -> Result<Option<Content>, ParseError> {
    let mut matches = fields
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(field))
        .cloned()
        .collect::<Vec<_>>();

    match matches.len() {
        0 => Ok(None),
        1 => Ok(fields.remove(&matches[0])),
        _ => {
            matches.sort();
            Err(ParseError::DuplicateField {
                path: path.into(),
                field: field.into(),
                keys: matches,
            })
        }
    }
}

/// The `content` field (§5.1.4, §5.2.4), and the one place base64 is undone.
fn take_content(
    fields: &mut Fields,
    format: Format,
    wire: Wire,
    path: &str,
) -> Result<Content, ParseError> {
    // Present-but-null is a value here, not an absence: Annex A's schema lists `null`
    // among the types `Content` may take, so the field is present and the question of
    // whether null means anything for this `format` belongs to `validate`.
    let Some(content) = take(fields, "content", path)? else {
        return Err(ParseError::MissingField {
            path: path.into(),
            field: "content",
        });
    };

    match (wire, format, content) {
        // §6.4 calls it "JSON Base64 encoding" and ECMA-432 §7.2 asks for base64 only
        // in the text fallback, so this decoding is a property of the JSON wire and of
        // `binary` — not of the content, and not of CBOR.
        (Wire::Json, Format::Binary, Content::String(text)) => base64_decode(&text)
            .map(Content::Bytes)
            .map_err(|source| ParseError::Base64 {
                path: path.into(),
                source,
            }),
        (_, _, content) => Ok(content),
    }
}

fn take_string(
    fields: &mut Fields,
    field: &'static str,
    path: &str,
) -> Result<Option<String>, ParseError> {
    // For the string-typed fields — `messagetype`, `label`, `format`, `subformat` — a
    // present-but-null field counts as absent. Unlike `content`, none of them has a
    // null in its type: §5.1.1 and §5.2.1 say "must be a string", so a null carries
    // nothing the standard could interpret, and rejecting an optional field for being
    // explicitly empty would fail messages nothing is wrong with.
    match take(fields, field, path)? {
        None | Some(Content::Null) => Ok(None),
        Some(Content::String(value)) => Ok(Some(value)),
        Some(other) => Err(ParseError::WrongType {
            path: path.into(),
            field: field.into(),
            expected: "a string",
            found: other.type_name(),
        }),
    }
}

fn take_required_string(
    fields: &mut Fields,
    field: &'static str,
    path: &str,
) -> Result<String, ParseError> {
    take_string(fields, field, path)?.ok_or_else(|| ParseError::MissingField {
        path: path.into(),
        field,
    })
}

fn take_bool(
    fields: &mut Fields,
    field: &'static str,
    path: &str,
) -> Result<Option<bool>, ParseError> {
    match take(fields, field, path)? {
        None | Some(Content::Null) => Ok(None),
        Some(Content::Bool(value)) => Ok(Some(value)),
        Some(other) => Err(ParseError::WrongType {
            path: path.into(),
            field: field.into(),
            expected: "a boolean",
            found: other.type_name(),
        }),
    }
}

fn take_format(fields: &mut Fields, field: &'static str, path: &str) -> Result<Format, ParseError> {
    let raw = take_required_string(fields, field, path)?;
    Format::parse(&raw).ok_or_else(|| ParseError::UnknownFormat {
        path: path.into(),
        value: raw,
        expected: Format::EXPECTED,
    })
}

fn starts_with_ignore_case(value: &str, prefixes: &[&str]) -> bool {
    // `get` rather than a byte slice, because `value` is the peer's string: a prefix
    // length that lands inside a multi-byte character is not an index to slice at, and
    // means "not this prefix" — not a panic. Five euro signs are fifteen bytes, and the
    // boundary for `authentication` (fourteen) falls inside the fifth.
    prefixes.iter().any(|prefix| {
        value
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

// ---------------------------------------------------------------------------
// Semantic validation (ECMA-430 §5.3)
// ---------------------------------------------------------------------------

/// Content/encoding pairs permitted inside a `binary` subformat (§5.3).
///
/// The table lists `audio`, `image`, `sensor` and `generic`, yet its own examples
/// mention `video/.mp3`. `video` is therefore accepted as well; rejecting it would
/// break peers that follow the examples rather than the table.
const BINARY_CONTENT_KINDS: [&str; 5] = ["audio", "image", "sensor", "generic", "video"];

fn validate_subformat(format: Format, subformat: &str, path: &str, findings: &mut Vec<Violation>) {
    if subformat.trim().is_empty() {
        findings.push(Violation::error(
            path,
            "ecma430-5.2.3/subformat-empty",
            "subformat is required and must not be empty",
        ));
        return;
    }

    match format {
        Format::Text => {
            // `<lang>`: any language tag is allowed, so nothing further to check.
        }
        Format::Token => {
            // `<prefix>_<suffix>`, where the suffix is optional.
        }
        Format::Structured => {
            // Informational, and it cannot be anything else: Table 1's fifth value is
            // `<prog>`, "the name of any programming language", which is a list no
            // decoder can hold, so `python` and ECMA-432 §8's `application/json` are
            // indistinguishable here. The finding says a subformat is not one of the
            // four fixed values and leaves the reading to whoever knows the deployment.
            const KNOWN: [&str; 4] = ["json", "uri", "xml", "html"];
            if !KNOWN.iter().any(|k| subformat.eq_ignore_ascii_case(k)) {
                findings.push(Violation::warning(
                    path,
                    "ecma430-5.3/structured-subformat",
                    format!(
                        "`{subformat}` is not one of json, uri, xml or html; Table 1 also \
                         permits the name of any programming language, which cannot be \
                         enumerated, so this finding is informational"
                    ),
                ));
            }
        }
        // Findings below are warnings rather than errors on purpose, and the ground is
        // this crate's own definition of the word: a `Warning` is off-spec but still
        // interpretable, and `webm` under `binary` names an encoding a reader can act
        // on even though Table 1 asks for `<content>/<encoding>`. TR/113 Figure 9
        // sends exactly that, which corroborates the reading without being its
        // authority — a Technical Report is informative. An empty encoding is the
        // Error below, because then there is nothing left to interpret.
        Format::Binary => match subformat.split_once('/') {
            None => findings.push(Violation::warning(
                path,
                "ecma430-5.3/binary-subformat",
                format!("`{subformat}` should be <content>/<encoding>, e.g. `image/jpeg`"),
            )),
            Some((kind, encoding)) => {
                if !BINARY_CONTENT_KINDS
                    .iter()
                    .any(|k| kind.eq_ignore_ascii_case(k))
                {
                    findings.push(Violation::warning(
                        path,
                        "ecma430-5.3/binary-content-kind",
                        format!(
                            "`{kind}` is normally one of {}",
                            BINARY_CONTENT_KINDS.join(", ")
                        ),
                    ));
                }
                if encoding.trim_matches('.').is_empty() {
                    findings.push(Violation::error(
                        path,
                        "ecma430-5.3/binary-encoding",
                        "encoding part of the subformat is empty, so the payload has no type",
                    ));
                } else if encoding.contains(';') {
                    // ECMA-432 §8 Example 3 — a clause that calls its examples
                    // normative — sends `"Subformat": "audio/wav;base64"`, where Table 1
                    // asks for "specific content encoding, e.g. bmp or jpeg". The
                    // parameter is reported so that it is visible, and tolerated for the
                    // same reason TR/113's `webm` is: the committee sent it.
                    findings.push(Violation::warning(
                        path,
                        "ecma430-5.3/binary-encoding-parameter",
                        format!(
                            "encoding `{encoding}` carries a parameter; Table 1 asks for \
                             a bare content encoding such as `wav`, and ECMA-432 §8 \
                             Example 3 sends `audio/wav;base64`, so this is accepted"
                        ),
                    ));
                }
            }
        },
        Format::Location => {
            if !subformat.eq_ignore_ascii_case("text") && !subformat.eq_ignore_ascii_case("GPS") {
                findings.push(Violation::warning(
                    path,
                    "ecma430-5.3/location-subformat",
                    format!("`{subformat}` is normally `text` or `GPS`"),
                ));
            }
        }
        Format::Generic => {
            // `<ext>`: private extension, any non-empty name.
        }
    }
}

fn validate_content(format: Format, content: &Content, path: &str, findings: &mut Vec<Violation>) {
    match format {
        Format::Text => {
            // `null` and a byte string fall in here with every other non-string: §5.3
            // says the content of a `text` submessage is natural language, and natural
            // language is nothing but a string. There is no reading under which an
            // object is text, so this is an Error by the crate's own definition of the
            // word — the message cannot be interpreted as written.
            if !content.is_string() {
                findings.push(Violation::error(
                    path,
                    "ecma430-5.1.4/text-content",
                    format!(
                        "content of a `{format}` submessage must be a string, found {}",
                        content.type_name()
                    ),
                ));
            }
        }
        Format::Token => {
            // A Warning, not the Error `text` gets, and under its own id. Table 1 calls
            // a token's content "an opaque token interpreted by end-points" and does
            // not say *string*; Annex A's schema allows `Content` any type; and §6.2's
            // echo — "the exact same format, subformat and content" — is as well
            // defined for a number as for a string. A non-string token is unusual, and
            // it is still interpretable, which is exactly what `Severity::Warning`
            // means.
            if !content.is_string() {
                findings.push(Violation::warning(
                    path,
                    "ecma430-5.3/token-content",
                    format!(
                        "content of a `token` submessage is normally a string, found {}; \
                         Table 1 calls it an opaque token and does not fix its type",
                        content.type_name()
                    ),
                ));
            }
        }
        Format::Binary => match content {
            // What ECMA-432 §7.1 asks for, and what a JSON base64 payload decodes to.
            Content::Bytes(_) => {}
            // Reachable only from CBOR or from a locally built message: a string under
            // `binary` on the JSON wire has already become bytes by the time validation
            // runs. Over CBOR the peer had a byte string available and did not use it.
            Content::String(_) => findings.push(Violation::warning(
                path,
                "ecma432-7.1/binary-content-text",
                "binary content arrived as text; over a binary-safe wire it should be a \
                 byte string (ECMA-432 §7.1)",
            )),
            // An Error, like a non-string `text` content and for the same reason:
            // §5.1.4 and §5.2.4 make the same demand of every format — content "must
            // be represented as specified by the format and subformat field" — and
            // Table 1 makes that bytes, spelled in base64 over JSON (§6.4). Nothing in
            // the text lets an object or a number be read as binary content. A payload
            // carried *by reference* is not the gap it looks like: §6.4 already defines
            // how a large payload is referenced — a `control` exchange answered with a
            // `structured` / `uri` submessage — and that is a different shape.
            other => findings.push(Violation::error(
                path,
                "ecma430-5.3/binary-content",
                format!(
                    "binary content is expected to be bytes, or base64 text over JSON, found {}",
                    other.type_name()
                ),
            )),
        },
        Format::Structured | Format::Location | Format::Generic => {
            // Shape follows from the subformat and is left to the endpoint.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Severity;
    use serde_json::json;

    fn parse(json: &str) -> Message {
        Message::from_json_bytes(json.as_bytes()).expect("message should parse")
    }

    #[test]
    fn decodes_canonical_lower_case() {
        let message = parse(r#"{"format":"text","subformat":"english","content":"hi"}"#);
        assert_eq!(message.format, Format::Text);
        assert_eq!(message.subformat, "english");
        assert_eq!(message.content, Content::String("hi".into()));
        assert!(message.submessages.is_empty());
    }

    #[test]
    fn field_names_are_case_insensitive() {
        let message = parse(r#"{"FORMAT":"TEXT","SubFormat":"English","CONTENT":"hi"}"#);
        assert_eq!(message.format, Format::Text);
        assert_eq!(message.subformat, "English");
    }

    #[test]
    fn capitalised_reference_schema_is_accepted() {
        // Annex A spells the fields in PascalCase; the standard's own schema must
        // be decodable by an implementation that follows the standard.
        let message = parse(
            r#"{"MessageType":"Request","Format":"text","Subformat":"english",
                "Content":"hi","Submessages":[{"Label":"role","Format":"text",
                "Subformat":"english","Content":"there"}]}"#,
        );
        assert_eq!(message.message_type.as_deref(), Some("Request"));
        assert_eq!(message.submessages.len(), 1);
        assert_eq!(message.submessages[0].label.as_deref(), Some("role"));
    }

    #[test]
    fn duplicate_field_with_different_casing_is_rejected() {
        let error = Message::from_json_bytes(
            br#"{"format":"text","Format":"text","subformat":"e","content":"x"}"#,
        )
        .expect_err("ambiguous document must not decode");
        assert!(
            matches!(error, ParseError::DuplicateField { .. }),
            "{error}"
        );
    }

    #[test]
    fn missing_required_field_is_named() {
        let error = Message::from_json_bytes(br#"{"format":"text","subformat":"english"}"#)
            .expect_err("content is required");
        assert_eq!(error.path(), "$");
        assert!(error.to_string().contains("content"), "{error}");
    }

    #[test]
    fn unknown_format_lists_the_allowed_values() {
        let error =
            Message::from_json_bytes(br#"{"format":"audio","subformat":"english","content":"x"}"#)
                .expect_err("`audio` is not a format value");
        assert!(matches!(error, ParseError::UnknownFormat { .. }), "{error}");
    }

    #[test]
    fn submessage_errors_carry_their_index() {
        let error = Message::from_json_bytes(
            br#"{"format":"text","subformat":"english","content":"x","submessages":[{"format":"text","subformat":"english","content":"ok"},{"format":"text","content":"missing subformat"}]}"#,
        )
        .expect_err("second submessage is malformed");
        assert_eq!(error.path(), "submessages[1]");
    }

    #[test]
    fn submessages_must_be_an_array() {
        // §5.1.5: "it must be represented as a array defined by ECMA-404".
        let error = Message::from_json_bytes(
            br#"{"format":"text","subformat":"english","content":"x","submessages":{"a":1}}"#,
        )
        .expect_err("an object is not an array");
        assert!(
            matches!(&error, ParseError::WrongType { field, found, .. }
                if &**field == "submessages" && *found == "an object"),
            "{error}"
        );
    }

    #[test]
    fn fields_the_standard_does_not_define_are_dropped() {
        // Clause 5 lists the fields a message has. An unknown one has no defined
        // meaning, so it is not carried into the model and not re-emitted.
        let message =
            parse(r#"{"format":"text","subformat":"english","content":"hi","session":"s-1"}"#);
        assert!(!message.to_json_string().contains("session"));
    }

    #[test]
    fn structured_content_is_not_case_folded() {
        // Only the protocol's own fields are case-insensitive; the payload is the
        // peer's data and must survive a round trip untouched.
        let message = parse(
            r#"{"format":"structured","subformat":"json","content":{"Key":"Value"},"submessages":[]}"#,
        );
        let encoded = message.to_json_string();
        assert!(encoded.contains(r#""Key":"Value""#), "{encoded}");
    }

    #[test]
    fn round_trip_preserves_order_and_content() {
        let source = r#"{"format":"text","subformat":"english","content":"first","submessages":[{"label":"b","format":"text","subformat":"english","content":"second"},{"format":"token","subformat":"conversation_srv","content":"abc"}]}"#;
        let message = parse(source);
        let reparsed = parse(&message.to_json_string());
        assert_eq!(message, reparsed);
    }

    #[test]
    fn control_is_recognised_in_both_spellings() {
        assert!(
            parse(
                r#"{"messagetype":"Control","format":"text","subformat":"english","content":"?"}"#
            )
            .is_control()
        );
        assert!(
            parse(r#"{"control":true,"format":"text","subformat":"english","content":"?"}"#)
                .is_control()
        );
        assert!(
            !parse(r#"{"control":false,"format":"text","subformat":"english","content":"?"}"#)
                .is_control()
        );
    }

    #[test]
    fn the_upload_request_is_a_control_text_message() {
        // §6.4 ¶2: "a message with the MessageType field set to 'control', and the
        // first submessage should include a text format request".
        let request = Message::upload_request("where do I send a large binary upload?");

        assert!(request.is_control());
        assert_eq!(request.message_type.as_deref(), Some("control"));
        assert_eq!(request.format, Format::Text);
        assert_eq!(request.subformat, "english");
        assert!(request.submessages.is_empty());
        assert!(request.validate().is_empty());
        assert_eq!(
            request.to_json_string(),
            r#"{"messagetype":"control","format":"text","subformat":"english","content":"where do I send a large binary upload?"}"#
        );
    }

    #[test]
    fn the_upload_response_carries_the_uri_in_a_submessage() {
        // §6.4 ¶3: "a message with MessageType field set to 'control' containing a
        // submessage with the format field set of 'structured' and subformat of a URI".
        let response = Message::upload_response("https://agent.example.test:8443/nlip/upload");

        assert!(response.is_control());
        assert_eq!(
            response.format,
            Format::Text,
            "the address is not the answer"
        );
        assert_eq!(response.submessages.len(), 1);
        assert_eq!(response.submessages[0].format, Format::Structured);
        assert_eq!(response.submessages[0].subformat, "uri");
        assert_eq!(
            response.submessages[0].content_as_str(),
            Some("https://agent.example.test:8443/nlip/upload")
        );
        assert!(response.validate().is_empty());
        assert_eq!(parse(&response.to_json_string()), response);
        assert_eq!(
            response.to_json_string(),
            concat!(
                r#"{"messagetype":"control","format":"text","subformat":"english","#,
                r#""content":"Send large binary content to the end-point named in the submessage below.","#,
                r#""submessages":[{"format":"structured","subformat":"uri","#,
                r#""content":"https://agent.example.test:8443/nlip/upload"}]}"#
            )
        );
    }

    #[test]
    fn the_6_4_constructors_do_not_invent_the_boolean_control_field() {
        // §6.4 names `MessageType`, §6.3 names the boolean: each clause writes its own
        // field, and `is_control` reads either. A constructor that set both would be
        // answering an obligation nobody gave it.
        let request = Message::upload_request("?");
        let response = Message::upload_response("https://agent.example.test/nlip/upload");

        assert!(request.control.is_none());
        assert!(response.control.is_none());
        assert!(!request.to_json_string().contains(r#""control":"#));
        assert!(!response.to_json_string().contains(r#""control":"#));
    }

    #[test]
    fn a_subformat_whose_prefix_length_falls_inside_a_character_is_not_ours() {
        // The subformat is the peer's string, and the classifier compares a byte
        // prefix of it. A multi-byte character straddling that boundary must read as
        // "not this prefix", not as a panic: five euro signs are fifteen bytes, so
        // the boundary for `authentication` (fourteen) lands inside the fifth.
        let subformat = "€€€€€";
        assert!(subformat.len() > "authentication".len());
        assert!(!Submessage::token(subformat, "x").is_authentication_token());
        assert!(!Submessage::token(subformat, "x").is_conversation_token());
    }

    #[test]
    fn token_subformats_are_classified_case_insensitively() {
        assert!(Submessage::token("Conversation_9.2.3.5", "x").is_conversation_token());
        assert!(Submessage::token("AUTHENTICATION_9.2.3.5", "x").is_authentication_token());
    }

    #[test]
    fn only_the_standards_two_prefixes_are_reserved() {
        // §6.2: a token subformat "can be any general string, and not necessarily
        // beginning with 'conversation' or 'authentication'. The use of these tokens is
        // up to the application, and may be used to support group identity". The
        // reference SDK's `authorization` is an application token by that reading —
        // still echoed (see `tokens_are_echoed…` in the HTTP binding's tests), but not
        // a credential this endpoint claims to understand.
        assert!(!Submessage::token("authorization", "x").is_authentication_token());
        assert!(!Submessage::token("Authorization_9.2.3.5", "x").is_authentication_token());
        assert!(!Submessage::token("conversation", "x").is_authentication_token());
        assert!(!Submessage::token("authentication", "x").is_conversation_token());
    }

    #[test]
    fn binary_subformat_shape_is_reported_but_tolerated() {
        // TR/113 Figure 9 uses exactly this shape, so it must not be fatal.
        let message = parse(r#"{"format":"binary","subformat":"webm","content":"AA=="}"#);
        let findings = message.validate();
        let finding = findings
            .iter()
            .find(|f| f.rule == "ecma430-5.3/binary-subformat")
            .expect("shape should be reported");
        assert_eq!(finding.severity, Severity::Warning);
    }

    #[test]
    fn the_committees_own_figure_9_payload_is_not_base64() {
        // TR/113 Figure 9 sends `"content": "AAAAAAAAAAAAAAAAAAAAA="` — twenty-one data
        // characters, a length no base64 encoder produces, since a group holds two,
        // three or four. Tolerating the figure's `webm` subformat is one thing: the
        // shape is unusual and the payload is still recoverable. A payload that does
        // not decode is another, and inventing bytes for it would be worse than saying
        // so.
        let error = Message::from_json_str(
            r#"{"control":false,"format":"binary","subformat":"webm","content":"AAAAAAAAAAAAAAAAAAAAA=","submessages":[]}"#,
        )
        .expect_err("twenty-one data characters is not base64");
        assert!(matches!(error, ParseError::Base64 { .. }), "{error}");
    }

    #[test]
    fn binary_without_an_encoding_is_fatal() {
        let message = parse(r#"{"format":"binary","subformat":"image/","content":"AA=="}"#);
        let findings = message.validate();
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "ecma430-5.3/binary-encoding" && f.severity == Severity::Error),
            "{findings:?}"
        );
    }

    #[test]
    fn empty_subformat_is_fatal_for_every_format() {
        for format in [
            "text",
            "token",
            "structured",
            "binary",
            "location",
            "generic",
        ] {
            let message = parse(&format!(
                r#"{{"format":"{format}","subformat":"","content":"AA=="}}"#
            ));
            assert!(
                message
                    .validate()
                    .iter()
                    .any(|f| f.severity == Severity::Error),
                "{format} with an empty subformat should be an error"
            );
        }
    }

    #[test]
    fn binary_content_kind_from_the_table_is_accepted() {
        for subformat in ["audio/bmp", "image/jpeg", "generic/.zip", "video/.mp3"] {
            let message = parse(&format!(
                r#"{{"format":"binary","subformat":"{subformat}","content":"AA=="}}"#
            ));
            assert!(message.validate().is_empty(), "{subformat}");
        }
    }

    #[test]
    fn location_accepts_only_text_and_gps() {
        assert!(
            parse(r#"{"format":"location","subformat":"GPS","content":"1,2"}"#)
                .validate()
                .is_empty()
        );
        let findings =
            parse(r#"{"format":"location","subformat":"plus-code","content":"1,2"}"#).validate();
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "ecma430-5.3/location-subformat")
        );
    }

    #[test]
    fn a_token_in_the_first_submessage_is_found_by_the_predicates_not_by_tokens() {
        // §6.2 ¶2 speaks of "any NLIP message containing a submessage" with format
        // `token`, and §5 ¶2 calls the leading object the first submessage. `tokens()`
        // cannot yield it — it is not a `Submessage` — so the predicates are how an
        // endpoint sees it at all.
        let message =
            parse(r#"{"format":"token","subformat":"authentication_9.2.3.5","content":"abc"}"#);
        assert!(message.is_token());
        assert!(message.is_authentication_token());
        assert!(!message.is_conversation_token());
        assert_eq!(message.tokens().count(), 0);

        let conversation = parse(r#"{"format":"token","subformat":"Conversation","content":"c"}"#);
        assert!(conversation.is_conversation_token());
        assert!(!conversation.is_authentication_token());

        // And a message that is not a token answers no to all three.
        let text = Message::text("hi");
        assert!(!text.is_token() && !text.is_authentication_token());
    }

    #[test]
    fn token_content_that_is_not_a_string_is_a_warning_of_its_own() {
        // Table 1 calls it "an opaque token interpreted by end-points" and does not say
        // string; Annex A allows any type; §6.2's echo is well defined for a number.
        let findings = parse(
            r#"{"format":"text","subformat":"english","content":"x","submessages":[{"format":"token","subformat":"conversation","content":9060312}]}"#,
        )
        .validate();
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "ecma430-5.3/token-content"
                    && f.severity == Severity::Warning
                    && f.path == "submessages[0]"),
            "{findings:?}"
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "ecma430-5.1.4/text-content"),
            "{findings:?}"
        );
    }

    #[test]
    fn binary_content_that_is_neither_bytes_nor_text_is_an_error() {
        // §5.1.4 makes the same demand of every format, and Table 1 makes `binary`
        // bytes. §6.4's by-reference mechanism is a `control` exchange with a
        // `structured`/`uri` submessage, not an object under `binary`.
        for content in [
            "{\"url\":\"https://x.test/1\"}",
            "[1,2]",
            "7",
            "true",
            "null",
        ] {
            let findings = parse(&format!(
                r#"{{"format":"binary","subformat":"image/jpeg","content":{content}}}"#
            ))
            .validate();
            assert!(
                findings.iter().any(
                    |f| f.rule == "ecma430-5.3/binary-content" && f.severity == Severity::Error
                ),
                "{content}: {findings:?}"
            );
        }
    }

    #[test]
    fn text_content_must_be_a_string() {
        let findings =
            parse(r#"{"format":"text","subformat":"english","content":{"a":1}}"#).validate();
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "ecma430-5.1.4/text-content")
        );
    }

    #[test]
    fn extract_text_joins_the_first_submessage_with_the_rest_in_order() {
        let message = parse(
            r#"{"format":"text","subformat":"english","content":"a","submessages":[{"format":"text","subformat":"english","content":"b"},{"format":"text","subformat":"english","content":"c"}]}"#,
        );
        assert_eq!(
            message.extract_text(Some("english")).as_deref(),
            Some("a b c")
        );
        assert_eq!(message.extract_text(None).as_deref(), Some("a b c"));
        assert_eq!(message.extract_text(Some("russian")), None);
    }

    #[test]
    fn extract_text_skips_other_formats_and_languages() {
        let message = parse(
            r#"{"format":"text","subformat":"english","content":"hello","submessages":[{"format":"text","subformat":"russian","content":"привет"},{"format":"structured","subformat":"json","content":{"a":1}}]}"#,
        );
        assert_eq!(
            message.extract_text(Some("english")).as_deref(),
            Some("hello")
        );
        assert_eq!(
            message.extract_text(Some("russian")).as_deref(),
            Some("привет")
        );
        assert_eq!(message.extract_text(None).as_deref(), Some("hello привет"));
    }

    #[test]
    fn extract_text_of_a_non_text_message_is_none() {
        let message = parse(r#"{"format":"structured","subformat":"json","content":{"a":1}}"#);
        assert_eq!(message.extract_text(None), None);
    }

    // -----------------------------------------------------------------------
    // Content, base64 and the wire
    // -----------------------------------------------------------------------

    #[test]
    fn binary_content_from_json_is_base64_and_comes_back_as_bytes() {
        // ECMA-432 §7.2: "Binary data shall be base64-encoded" in the text fallback.
        let message = parse(r#"{"format":"binary","subformat":"audio/wav","content":"UklGRg=="}"#);
        assert_eq!(message.content, Content::Bytes(b"RIFF".to_vec()));
        assert_eq!(message.content_as_bytes(), Some(&b"RIFF"[..]));
        // …and back out again in the same spelling, so a JSON peer sees no change.
        assert!(
            message.to_json_string().contains(r#""content":"UklGRg==""#),
            "{}",
            message.to_json_string()
        );
        assert_eq!(message.to_value()["content"], json!("UklGRg=="));
    }

    #[test]
    fn only_binary_content_is_treated_as_base64() {
        // A `structured` payload that happens to look like base64 is a string. The rule
        // is about `format`, not about what a string could be read as.
        let message = parse(r#"{"format":"structured","subformat":"json","content":"UklGRg=="}"#);
        assert_eq!(message.content, Content::String("UklGRg==".into()));
    }

    #[test]
    fn binary_content_that_is_not_base64_is_a_parse_error() {
        let error = Message::from_json_str(
            r#"{"format":"binary","subformat":"audio/wav","content":"not base64!!"}"#,
        )
        .expect_err("the payload cannot be interpreted");
        assert!(matches!(error, ParseError::Base64 { .. }), "{error}");
        assert_eq!(error.path(), "$");
    }

    #[test]
    fn a_bad_base64_submessage_names_its_index() {
        let error = Message::from_json_str(
            r#"{"format":"text","subformat":"english","content":"x","submessages":[
                {"format":"text","subformat":"english","content":"ok"},
                {"format":"binary","subformat":"image/jpeg","content":"!!!"}]}"#,
        )
        .expect_err("the second submessage is not base64");
        assert!(matches!(error, ParseError::Base64 { .. }), "{error}");
        assert_eq!(error.path(), "submessages[1]");
    }

    #[test]
    fn unpadded_base64_is_accepted() {
        // §6.4 says "JSON Base64 encoding" and does not say "padded"; rejecting this
        // would invent a rule and drop a message whose bytes are entirely recoverable.
        let message = parse(r#"{"format":"binary","subformat":"audio/wav","content":"UklGRg"}"#);
        assert_eq!(message.content, Content::Bytes(b"RIFF".to_vec()));
        // Encoding is not equally relaxed: what goes back out is padded.
        assert!(message.to_json_string().contains("UklGRg=="));
    }

    #[test]
    fn null_content_is_a_value_and_a_finding_rather_than_a_missing_field() {
        // Annex A's schema lists `null` among the types of `Content`, so the field is
        // present. Whether null means anything for a `text` message is `validate`'s
        // question, and its answer is no.
        let message = parse(r#"{"format":"text","subformat":"english","content":null}"#);
        assert_eq!(message.content, Content::Null);
        assert!(
            message
                .validate()
                .iter()
                .any(|finding| finding.rule == "ecma430-5.1.4/text-content"
                    && finding.severity == Severity::Error),
            "{:?}",
            message.validate()
        );
    }

    #[test]
    fn a_null_string_field_still_counts_as_absent() {
        // The other side of the same coin: §5.1.2 says `format` "must be a string", so
        // a null there carries nothing, and the field is required.
        let error =
            Message::from_json_str(r#"{"format":null,"subformat":"english","content":"hi"}"#)
                .expect_err("format is required");
        assert!(matches!(error, ParseError::MissingField { .. }), "{error}");

        // An optional one simply stays `None` rather than failing.
        let message = parse(
            r#"{"messagetype":null,"label":null,"control":null,
            "format":"text","subformat":"english","content":"hi"}"#,
        );
        assert_eq!(message.message_type, None);
        assert_eq!(message.label, None);
        assert_eq!(message.control, None);
    }

    #[test]
    fn binary_content_built_locally_keeps_its_bytes() {
        let message = Message::binary("image/jpeg", vec![0xff, 0xd8])
            .with_submessage(Submessage::binary("audio/wav", b"RIFF".to_vec()))
            .with_submessage(Submessage::text("look at this"));
        assert_eq!(message.content_as_bytes(), Some(&[0xff, 0xd8][..]));
        assert_eq!(
            message.submessages[0].content_as_bytes(),
            Some(&b"RIFF"[..])
        );
        assert_eq!(
            message.submessages[1].content_as_str(),
            Some("look at this")
        );
        assert_eq!(message.validate(), Vec::new());
        // A JSON round trip is lossless, through base64 and back.
        assert_eq!(parse(&message.to_json_string()), message);
    }

    #[test]
    fn constructors_take_anything_that_is_content() {
        // `serde_json::Value` still works, which is what the constructors used to take.
        let from_value = Message::new(Format::Text, "english", json!("hi"));
        assert_eq!(from_value, Message::text("hi"));
        assert_eq!(
            Submessage::new(Format::Text, "english", "hi"),
            Submessage::text("hi")
        );
    }

    #[test]
    fn format_parses_from_a_string_both_ways() {
        assert_eq!("TEXT".parse::<Format>().unwrap(), Format::Text);
        assert_eq!(Format::parse("Binary"), Some(Format::Binary));
        let error = "audio".parse::<Format>().expect_err("not a Table 1 value");
        assert!(matches!(error, ParseError::UnknownFormat { .. }), "{error}");
    }

    // -----------------------------------------------------------------------
    // serde
    // -----------------------------------------------------------------------

    #[test]
    fn serde_and_the_direct_entry_points_agree() {
        let source = r#"{"messagetype":"Request","format":"binary","subformat":"audio/wav;base64","content":"UklGRg==","submessages":[{"label":"transcription","format":"text","subformat":"en-US","content":"hi"}]}"#;
        let direct = Message::from_json_str(source).expect("direct decode");
        let through_serde: Message = serde_json::from_str(source).expect("serde decode");
        assert_eq!(direct, through_serde);
        assert_eq!(
            serde_json::to_string(&direct).unwrap(),
            direct.to_json_string()
        );
        assert_eq!(direct.to_json_vec(), direct.to_json_string().into_bytes());
    }

    #[test]
    fn the_serde_path_reports_the_same_failure_as_a_string() {
        let error = serde_json::from_str::<Message>(r#"{"format":"text","subformat":"english"}"#)
            .expect_err("content is required");
        assert!(
            error.to_string().contains("required field `content`"),
            "{error}"
        );
    }

    #[test]
    fn a_submessage_decodes_on_its_own() {
        let submessage = Submessage::from_content(
            Content::from(
                json!({"format": "binary", "subformat": "image/jpeg", "content": "UklGRg=="}),
            ),
            Wire::Json,
        )
        .expect("submessage should decode");
        assert_eq!(submessage.content, Content::Bytes(b"RIFF".to_vec()));
        assert_eq!(
            serde_json::from_str::<Submessage>(
                r#"{"format":"binary","subformat":"image/jpeg","content":"UklGRg=="}"#
            )
            .unwrap(),
            submessage
        );
    }

    // -----------------------------------------------------------------------
    // The examples of ECMA-432 §8, which that clause calls normative
    // -----------------------------------------------------------------------

    #[test]
    fn the_websocket_examples_are_accepted_with_a_warning() {
        // Example 1 sends `"Subformat": "application/json"` under `structured`, where
        // Table 1 says `json`; Example 3 sends `"audio/wav;base64"` under `binary`,
        // where Table 1 asks for a bare content encoding. Both are the committee's own
        // normative examples, so neither is an error — but both are reported.
        let structured = parse(
            r#"{"format":"structured","subformat":"application/json","content":{"intent":"weather_query"}}"#,
        );
        let findings = structured.validate();
        assert!(
            findings.iter().all(|f| f.severity == Severity::Warning)
                && findings
                    .iter()
                    .any(|f| f.rule == "ecma430-5.3/structured-subformat"),
            "{findings:?}"
        );

        let binary =
            parse(r#"{"format":"binary","subformat":"audio/wav;base64","content":"UklGRg=="}"#);
        let findings = binary.validate();
        assert!(
            findings.iter().all(|f| f.severity == Severity::Warning)
                && findings
                    .iter()
                    .any(|f| f.rule == "ecma430-5.3/binary-encoding-parameter"),
            "{findings:?}"
        );
    }

    // -----------------------------------------------------------------------
    // CBOR — ECMA-432 §7.1
    // -----------------------------------------------------------------------

    #[cfg(feature = "cbor")]
    mod cbor {
        use super::*;
        use ciborium::value::Value as CborValue;

        /// A type that records what the deserializer said about itself.
        #[derive(Debug)]
        struct Probe(bool);

        impl<'de> Deserialize<'de> for Probe {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Probe, D::Error> {
                let human_readable = deserializer.is_human_readable();
                Content::deserialize(deserializer)?;
                Ok(Probe(human_readable))
            }
        }

        fn cbor_message(content: CborValue) -> Vec<u8> {
            let message = CborValue::Map(vec![
                (
                    CborValue::Text("Format".into()),
                    CborValue::Text("binary".into()),
                ),
                (
                    CborValue::Text("Subformat".into()),
                    CborValue::Text("audio/wav".into()),
                ),
                (CborValue::Text("Content".into()), content),
            ]);
            let mut frame = Vec::new();
            ciborium::ser::into_writer(&message, &mut frame).expect("a Value encodes");
            frame
        }

        #[test]
        fn the_codecs_disagree_about_being_human_readable_and_that_is_the_whole_rule() {
            // `Wire::Json` versus `Wire::Cbor` rests on this answer, so it is worth a
            // test of its own rather than an assumption in the `Deserialize` impl.
            let mut frame = Vec::new();
            ciborium::ser::into_writer(&Content::Null, &mut frame).unwrap();
            let Probe(cbor) = ciborium::de::from_reader(&frame[..]).unwrap();
            assert!(!cbor, "ciborium must not claim to be human-readable");
            let Probe(json) = serde_json::from_str("null").unwrap();
            assert!(json, "serde_json must claim to be human-readable");
        }

        #[test]
        fn bytes_travel_as_a_cbor_byte_string_and_never_as_base64() {
            let message = Message::binary("audio/wav", b"RIFF".to_vec());
            let frame = message.to_cbor_vec();

            let at = frame
                .windows(4)
                .position(|window| window == b"RIFF")
                .expect("the raw payload is in the frame");
            // RFC 8949 §3.1: major type 2, length 4 — one byte, 0b010_00100.
            assert_eq!(frame[at - 1], 0x44, "{frame:02x?}");
            assert!(
                !frame.windows(6).any(|window| window == b"UklGRg"),
                "base64 must not appear on a binary wire: {frame:02x?}"
            );
            assert_eq!(Message::from_cbor_slice(&frame).unwrap(), message);
        }

        #[test]
        fn a_json_message_transcodes_to_cbor_and_back_unchanged() {
            // ECMA-432 §8 Example 3 is the text fallback of Example 1's audio: the same
            // message, spelled in base64 because JSON has no byte string. Reading it and
            // writing it as CBOR must produce §7.1's byte string, and coming back the
            // other way must produce the base64 again.
            let source = r#"{"MessageType":"Request","Format":"binary","Subformat":"audio/wav;base64","Content":"UklGRngAAAA=","Submessages":[{"Label":"transcription","Format":"text","Subformat":"en-US","Content":"What's the current stock price of Tesla?"}]}"#;
            let from_json = Message::from_json_str(source).expect("Example 3 decodes");
            assert!(from_json.content.is_bytes());

            let frame = from_json.to_cbor_vec();
            let from_cbor = Message::from_cbor_slice(&frame).expect("the frame decodes");
            assert_eq!(from_cbor, from_json);
            assert_eq!(from_cbor.to_value(), from_json.to_value());
            assert_eq!(
                from_cbor.to_value()["content"],
                serde_json::json!("UklGRngAAAA=")
            );
        }

        #[test]
        fn binary_content_sent_as_cbor_text_is_reported_but_kept() {
            // On this wire a text string is text: §7.1 offers a byte string, and the
            // peer did not use it. The payload is still delivered — it may well be
            // base64 the peer forgot to drop — and `validate` says so.
            let frame = cbor_message(CborValue::Text("UklGRg==".into()));
            let message = Message::from_cbor_slice(&frame).expect("the frame decodes");
            assert_eq!(message.content, Content::String("UklGRg==".into()));
            let findings = message.validate();
            assert!(
                findings
                    .iter()
                    .any(|finding| finding.rule == "ecma432-7.1/binary-content-text"
                        && finding.severity == Severity::Warning),
                "{findings:?}"
            );
        }

        #[test]
        fn a_tagged_content_item_is_rejected_rather_than_silently_untagged() {
            // RFC 8949 §3.4 tags carry semantics — tag 0 is a date-time string — that
            // NLIP has no field for and JSON has no spelling for. Stripping the tag
            // would hand the application a value whose meaning had quietly changed, so
            // the message is refused instead.
            let frame = cbor_message(CborValue::Tag(
                0,
                Box::new(CborValue::Text("2024-01-01T00:00:00Z".into())),
            ));
            let error = Message::from_cbor_slice(&frame).expect_err("a tag is not NLIP content");
            assert!(matches!(error, ParseError::Cbor(_)), "{error}");
            assert!(error.to_string().contains("enum"), "{error}");
        }

        #[test]
        fn a_bignum_that_fits_the_ecma_404_number_space_is_a_number() {
            // The one tag ciborium resolves for us (RFC 8949 §3.4.3): it arrives as an
            // integer, and is kept when ECMA-404 can represent it.
            let frame = cbor_message(CborValue::Tag(
                2,
                Box::new(CborValue::Bytes(vec![0x01, 0x00])),
            ));
            let message = Message::from_cbor_slice(&frame).expect("a bignum decodes");
            assert_eq!(message.content, Content::Number(256u64.into()));
        }

        #[test]
        fn a_frame_carrying_two_messages_is_refused() {
            // ECMA-432 §7.1: a binary message "shall contain a single NLIP message
            // encoded in CBOR". Decoding the first and dropping the second would make
            // the CBOR wire quietly accept what the JSON wire refuses, since
            // `serde_json` already rejects trailing characters.
            let one = Message::text("first").to_cbor_vec();
            let two = Message::text("second").to_cbor_vec();
            let mut frame = one.clone();
            frame.extend_from_slice(&two);

            let error = Message::from_cbor_slice(&frame).expect_err("two messages, one frame");
            assert!(
                matches!(error, ParseError::TrailingBytes { remaining } if remaining == two.len()),
                "{error}"
            );
            assert_eq!(error.path(), "$");
            // The single frame still decodes, and a trailing byte that is not even a
            // CBOR item is refused the same way.
            assert_eq!(
                Message::from_cbor_slice(&one).unwrap(),
                Message::text("first")
            );
            let mut ragged = one.clone();
            ragged.push(0x00);
            assert!(
                matches!(
                    Message::from_cbor_slice(&ragged),
                    Err(ParseError::TrailingBytes { remaining: 1 })
                ),
                "one stray byte is still more than a single message"
            );
        }

        #[test]
        fn a_non_finite_float_in_content_does_not_decode() {
            // RFC 8949 §3.3 can spell +Inf; ECMA-404 cannot, so the frame is refused
            // rather than delivered as a null the peer never sent.
            let frame = cbor_message(CborValue::Float(f64::INFINITY));
            let error = Message::from_cbor_slice(&frame).expect_err("+Inf is not ECMA-404 content");
            assert!(matches!(error, ParseError::Cbor(_)), "{error}");
            assert!(error.to_string().contains("outside the range"), "{error}");
        }

        #[test]
        fn cbor_that_is_not_cbor_at_all_is_a_typed_error() {
            let error =
                Message::from_cbor_slice(&[0xff, 0xff, 0xff]).expect_err("not a CBOR document");
            assert!(matches!(error, ParseError::Cbor(_)), "{error}");
            assert_eq!(error.path(), "$");
        }
    }
}
