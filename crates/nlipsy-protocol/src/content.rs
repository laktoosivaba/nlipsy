//! The content of an NLIP message, independent of the wire it travelled on.
//!
//! ECMA-430 §5.1.4 leaves the shape of `content` to the `format` and `subformat`
//! fields, and the schema in Annex A enumerates what a JSON document may put there:
//! string, number, object, array, boolean, null. ECMA-432 §7.1 adds one spelling
//! that JSON has no room for — over WebSocket a message is CBOR, and "CBOR Content
//! fields may include … Byte string (raw binary)"; §8 Example 1 sends the audio
//! submessage as `b'\x52\x49\x46\x46...'` and says in so many words that it "uses a
//! byte string directly and no base64 encoding".
//!
//! [`Content`] is the union of the two: the ECMA-404 value space plus a byte string.
//! Base64 is deliberately *not* part of it. ECMA-430 §6.4 calls base64 "JSON Base64
//! encoding" and ECMA-432 §7.2 asks for it only in the text fallback, so base64 is
//! how JSON *spells* a byte string rather than what a byte string *is*. The crate
//! undoes the spelling on the way in and redoes it on the way out, which is what lets
//! the same [`Message`](crate::Message) be read from a JSON frame and written to a
//! CBOR one without the payload turning into base64 of base64.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};

/// The base64 engine used for the JSON spelling of a byte string.
///
/// Encoding emits the standard alphabet with padding — the spelling of ECMA-432 §8
/// Example 3, whose `Content` ends in `...` but whose alphabet and padding are the
/// standard ones, and the only spelling every base64 reader accepts.
///
/// Decoding is deliberately more liberal than encoding: [`DecodePaddingMode::Indifferent`]
/// accepts the payload with or without its trailing `=`. Neither ECMA-430 §6.4 ("JSON
/// Base64 encoding") nor ECMA-432 §7.2 ("Binary data shall be base64-encoded") says
/// *padded*, so rejecting an unpadded but otherwise well-formed payload would be
/// enforcing a rule the standard does not state — and would drop a message whose bytes
/// are entirely recoverable. What is *not* relaxed is the alphabet (URL-safe `-_` is a
/// different encoding, not a lenient spelling of this one) and the trailing-bits check,
/// so a payload that cannot have been produced by a base64 encoder is still rejected.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_encode_padding(true)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Encode bytes the way JSON spells them (§6.4).
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

/// Decode the JSON spelling of a byte string (§6.4).
pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
    BASE64.decode(text)
}

/// The wire encoding a message came from, or goes to.
///
/// The only thing it changes is how `binary` content is spelled. Everything else about
/// an NLIP message — which fields exist, that their names are case-insensitive, that
/// the order of `submessages` is significant — is clause 5 of ECMA-430 and is the same
/// on every transport. ECMA-432 §7.1 carries a message as CBOR, where binary content is
/// a byte string; §7.2 falls back to JSON, where it is base64 text. A decoder cannot
/// tell those two apart from the document alone: a JSON string under
/// `format: "binary"` is base64, and a CBOR text string under the same format is not,
/// and only the frame that carried it knows which.
///
/// Transcoding does not touch the `subformat`: ECMA-432 §8 sends the same audio as
/// `audio/wav` over CBOR (Example 1) and as `audio/wav;base64` over JSON (Example 3),
/// so reading the second and writing it as CBOR leaves `;base64` standing over a byte
/// string that is not base64. The standard nowhere says to strip the parameter, and
/// rewriting a peer's subformat would be inventing a rule; a binding that transcodes
/// should know it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wire {
    /// UTF-8 JSON — ECMA-430's own representation and ECMA-432 §7.2's text fallback.
    /// Binary content is base64 text.
    Json,
    /// CBOR — ECMA-432 §7.1, the WebSocket binding's binary frame. Binary content is a
    /// CBOR byte string (major type 2) and is never base64.
    Cbor,
}

/// A content value, independent of the encoding it arrived in — ECMA-430 §5.1.4.
///
/// The variants are the six JSON types of Annex A's schema plus [`Bytes`](Content::Bytes),
/// the CBOR byte string of ECMA-432 §7.1. Converting to and from
/// [`serde_json::Value`] is lossless in one direction and lossy in the other by
/// construction: every `Value` is a `Content`, while a `Content::Bytes` becomes a
/// base64 string because that is all JSON can hold (§6.4).
///
/// The enum is **not** `#[non_exhaustive]`, unlike the message types. Its variants are
/// closed by the standard rather than by this crate: Annex A enumerates the JSON types
/// and ECMA-432 §7.1 adds exactly one more spelling. A seventh variant would have to
/// come from a revision of ECMA-430 — which breaks every implementation, not just this
/// one — so a `match` over `Content` is allowed to be exhaustive and to keep telling
/// its author when it has missed a case.
///
/// An object is a [`BTreeMap`] because ECMA-404 defines an object as an *unordered*
/// collection: nothing in a message may depend on the order its keys arrived in, in
/// contrast to `submessages`, whose order §5.1.5 makes significant. Sorting therefore
/// throws nothing away and buys determinism — the same content re-encodes to the same
/// bytes every time, which is what makes comparing two encodings byte for byte (a
/// conformance corpus, a signature, a cache key) mean anything.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Content {
    /// JSON `null`, CBOR `null`. Allowed by Annex A's schema, which lists `null` among
    /// the types of `Content`, so it decodes rather than being read as an absent field
    /// — whether it is *meaningful* for the message's format is
    /// [`Message::validate`](crate::Message::validate)'s question, not the decoder's.
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// A number. [`serde_json::Number`] is reused rather than reinvented: it already
    /// models exactly the ECMA-404 number space (an integer that fits `i64` or `u64`,
    /// otherwise a finite `f64`), and it is already a dependency.
    Number(Number),
    /// A text string. Over [`Wire::Json`] a `binary` message's string is base64 and is
    /// decoded into [`Bytes`](Content::Bytes) instead; see the module documentation.
    String(String),
    /// A byte string — ECMA-432 §7.1. Encodes as a CBOR byte string on a binary wire
    /// and as base64 text in JSON.
    Bytes(Vec<u8>),
    /// An ordered list of values.
    Array(Vec<Content>),
    /// An object. Keys are text: ECMA-430 §5 makes a message an ECMA-404 JSON object,
    /// and ECMA-404 has no other kind of key.
    Object(BTreeMap<String, Content>),
}

impl Content {
    /// The content as text, if it is a text string.
    ///
    /// A [`Bytes`](Content::Bytes) value is *not* rendered as base64 here: the two are
    /// different values, and a caller asking for text has not asked for a payload it
    /// would then have to decode again.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Content::String(text) => Some(text),
            _ => None,
        }
    }

    /// The content as bytes, if it is a byte string.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Content::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The content as a boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Content::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The content as a slice of values, if it is an array.
    pub fn as_array(&self) -> Option<&[Content]> {
        match self {
            Content::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The content as a map, if it is an object.
    pub fn as_object(&self) -> Option<&BTreeMap<String, Content>> {
        match self {
            Content::Object(fields) => Some(fields),
            _ => None,
        }
    }

    /// `true` for [`Content::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Content::Null)
    }

    /// `true` for [`Content::String`].
    pub fn is_string(&self) -> bool {
        matches!(self, Content::String(_))
    }

    /// `true` for [`Content::Bytes`].
    pub fn is_bytes(&self) -> bool {
        matches!(self, Content::Bytes(_))
    }

    /// The value as JSON, with bytes spelled as base64 (§6.4).
    ///
    /// This is the lossy direction: `Content::Bytes(b"hi")` and
    /// `Content::String("aGk=")` produce the same `Value`, because JSON cannot tell
    /// them apart either. Which one comes back out of a decoder is decided by the
    /// message's `format` and the [`Wire`] it arrived on.
    pub fn to_json_value(&self) -> Value {
        // Written over a reference rather than by cloning into `From<Content>`, because
        // the common caller has a `&Message` and wants a `Value` out of it.
        match self {
            Content::Null => Value::Null,
            Content::Bool(value) => Value::Bool(*value),
            Content::Number(number) => Value::Number(number.clone()),
            Content::String(text) => Value::String(text.clone()),
            Content::Bytes(bytes) => Value::String(base64_encode(bytes)),
            Content::Array(items) => {
                Value::Array(items.iter().map(Content::to_json_value).collect())
            }
            Content::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json_value()))
                    .collect::<Map<String, Value>>(),
            ),
        }
    }

    /// The name of this kind of value, with its article, for error prose.
    ///
    /// The article is part of the string because every use site reads `found {…}`.
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Content::Null => "null",
            Content::Bool(_) => "a boolean",
            Content::Number(_) => "a number",
            Content::String(_) => "a string",
            Content::Bytes(_) => "a byte string",
            Content::Array(_) => "an array",
            Content::Object(_) => "an object",
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<Value> for Content {
    fn from(value: Value) -> Content {
        match value {
            Value::Null => Content::Null,
            Value::Bool(value) => Content::Bool(value),
            Value::Number(number) => Content::Number(number),
            Value::String(text) => Content::String(text),
            Value::Array(items) => Content::Array(items.into_iter().map(Content::from).collect()),
            Value::Object(fields) => Content::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, Content::from(value)))
                    .collect(),
            ),
        }
    }
}

impl From<Content> for Value {
    fn from(content: Content) -> Value {
        match content {
            Content::Null => Value::Null,
            Content::Bool(value) => Value::Bool(value),
            Content::Number(number) => Value::Number(number),
            Content::String(text) => Value::String(text),
            // The JSON spelling of a byte string — §6.4, ECMA-432 §7.2.
            Content::Bytes(bytes) => Value::String(base64_encode(&bytes)),
            Content::Array(items) => Value::Array(items.into_iter().map(Value::from).collect()),
            Content::Object(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, Value::from(value)))
                    .collect::<Map<String, Value>>(),
            ),
        }
    }
}

impl From<&str> for Content {
    fn from(text: &str) -> Content {
        Content::String(text.to_owned())
    }
}

impl From<String> for Content {
    fn from(text: String) -> Content {
        Content::String(text)
    }
}

impl From<Vec<u8>> for Content {
    fn from(bytes: Vec<u8>) -> Content {
        Content::Bytes(bytes)
    }
}

impl From<&[u8]> for Content {
    fn from(bytes: &[u8]) -> Content {
        Content::Bytes(bytes.to_vec())
    }
}

impl From<bool> for Content {
    fn from(value: bool) -> Content {
        Content::Bool(value)
    }
}

impl From<i64> for Content {
    fn from(value: i64) -> Content {
        Content::Number(value.into())
    }
}

impl From<u64> for Content {
    fn from(value: u64) -> Content {
        Content::Number(value.into())
    }
}

impl From<f64> for Content {
    /// A non-finite float becomes [`Content::Null`].
    ///
    /// ECMA-404 has no NaN and no infinity, so there is nothing for them to convert
    /// into; `serde_json` resolves the same problem the same way, and a conversion that
    /// panicked would turn a peer's arithmetic into this crate's crash.
    fn from(value: f64) -> Content {
        Number::from_f64(value).map_or(Content::Null, Content::Number)
    }
}

impl From<Vec<Content>> for Content {
    fn from(items: Vec<Content>) -> Content {
        Content::Array(items)
    }
}

impl From<BTreeMap<String, Content>> for Content {
    fn from(fields: BTreeMap<String, Content>) -> Content {
        Content::Object(fields)
    }
}

// ---------------------------------------------------------------------------
// serde
//
// One pair of impls serves both codecs. What separates them is
// `Serializer::is_human_readable`, which serde_json answers `true` and ciborium
// `false` — exactly the JSON/CBOR split of ECMA-432 §7.1 and §7.2.
// ---------------------------------------------------------------------------

impl Serialize for Content {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Content::Null => serializer.serialize_unit(),
            Content::Bool(value) => serializer.serialize_bool(*value),
            // `Number` picks `serialize_i64`, `serialize_u64` or `serialize_f64` for
            // itself, so the codec — not this crate — decides how a number is framed.
            Content::Number(number) => number.serialize(serializer),
            Content::String(text) => serializer.serialize_str(text),
            Content::Bytes(bytes) => {
                if serializer.is_human_readable() {
                    serializer.serialize_str(&base64_encode(bytes))
                } else {
                    serializer.serialize_bytes(bytes)
                }
            }
            Content::Array(items) => {
                let mut sequence = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    sequence.serialize_element(item)?;
                }
                sequence.end()
            }
            Content::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Content {
    /// Decodes whatever the codec offers, without ever decoding base64.
    ///
    /// Base64 is a property of one field of one message — `content`, when `format` is
    /// `binary` and the wire is JSON — and this impl has none of that context: it is
    /// also what reads a `structured` payload, where a string that happens to be valid
    /// base64 is just a string. The rule therefore lives in
    /// [`Message::from_content`](crate::Message::from_content) and nowhere else.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Content, D::Error> {
        deserializer.deserialize_any(ContentVisitor)
    }
}

struct ContentVisitor;

impl<'de> Visitor<'de> for ContentVisitor {
    type Value = Content;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any NLIP content value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Content, E> {
        Ok(Content::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Content, E> {
        Ok(Content::Number(value.into()))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Content, E> {
        Ok(Content::Number(value.into()))
    }

    // CBOR can carry an integer that no ECMA-404 number can represent (RFC 8949 §3.1
    // major types 0 and 1 reach 2^64-1 in both directions, and a bignum reaches
    // further). Rounding it into an `f64` would silently change the peer's value, so
    // it is rejected: a message whose content cannot be represented has not been
    // decoded, whatever a lossy result might look like.
    fn visit_i128<E: de::Error>(self, value: i128) -> Result<Content, E> {
        if let Ok(value) = i64::try_from(value) {
            return Ok(Content::Number(value.into()));
        }
        if let Ok(value) = u64::try_from(value) {
            return Ok(Content::Number(value.into()));
        }
        Err(E::custom(format!(
            "integer {value} is outside the range an ECMA-404 number can represent"
        )))
    }

    fn visit_u128<E: de::Error>(self, value: u128) -> Result<Content, E> {
        u64::try_from(value)
            .map(|value| Content::Number(value.into()))
            .map_err(|_| {
                E::custom(format!(
                    "integer {value} is outside the range an ECMA-404 number can represent"
                ))
            })
    }

    /// A non-finite float is refused, for the reason the integer cases above are.
    ///
    /// CBOR carries an IEEE 754 NaN or ±∞ as an ordinary float (RFC 8949 §3.3), and
    /// ECMA-404 — which ECMA-430 §5 ¶4 makes the type system of a message — has
    /// neither. Reading one as [`Content::Null`] would hand the application a value
    /// the peer did not send, and a sensor reading of infinity would arrive as "no
    /// reading": a message whose content cannot be represented has not been decoded.
    /// [`From<f64>`](Content) is the other case and keeps its null — it builds a
    /// local value rather than reading a peer's.
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Content, E> {
        Number::from_f64(value).map(Content::Number).ok_or_else(|| {
            E::custom(format!(
                "float {value} is outside the range an ECMA-404 number can represent"
            ))
        })
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Content, E> {
        Ok(Content::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Content, E> {
        Ok(Content::String(value))
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Content, E> {
        Ok(Content::Bytes(value.to_vec()))
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Content, E> {
        Ok(Content::Bytes(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Content, E> {
        Ok(Content::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Content, E> {
        Ok(Content::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Content, D::Error> {
        Deserialize::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Content, A::Error> {
        let mut items = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
        while let Some(item) = sequence.next_element()? {
            items.push(item);
        }
        Ok(Content::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Content, A::Error> {
        // A repeated key keeps the last value, which is what serde_json does with a
        // repeated key in a JSON object. Note this is *content*, not the message's own
        // fields: §5's case-insensitive names and the duplicate-field rejection that
        // goes with them apply to the message, not to the peer's payload.
        let mut fields = BTreeMap::new();
        while let Some((ObjectKey(key), value)) = map.next_entry::<ObjectKey, Content>()? {
            fields.insert(key, value);
        }
        Ok(Content::Object(fields))
    }
}

/// An object key, which NLIP requires to be text.
///
/// CBOR allows a map key of any type (RFC 8949 §3.1), but ECMA-430 §5 says a message
/// "is primarily represented as a JSON message" and that "all types (arrays, strings,
/// objects etc.) are represented as defined in ECMA-404" — where an object's members
/// are `string: value` pairs. A CBOR map with, say, integer keys is therefore outside
/// NLIP: accepting it would mean inventing a JSON spelling for it on the way back out.
struct ObjectKey(String);

impl<'de> Deserialize<'de> for ObjectKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<ObjectKey, D::Error> {
        // `deserialize_any` rather than `deserialize_string`, so that a non-text key is
        // reported as the type it actually is instead of as whatever the codec makes of
        // a failed string request.
        deserializer.deserialize_any(ObjectKeyVisitor)
    }
}

struct ObjectKeyVisitor;

impl Visitor<'_> for ObjectKeyVisitor {
    type Value = ObjectKey;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a text string: NLIP object keys are ECMA-404 object names")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<ObjectKey, E> {
        Ok(ObjectKey(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<ObjectKey, E> {
        Ok(ObjectKey(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::IntoDeserializer as _;
    use serde::de::value::{Error as ValueError, MapDeserializer, StrDeserializer};
    use serde_json::json;

    #[test]
    fn every_json_value_survives_a_round_trip_through_content() {
        let value = json!({
            "text": "hi",
            "number": 1,
            "float": 1.5,
            "flag": true,
            "nothing": null,
            "list": [1, "two", {"three": 3}],
        });
        let content = Content::from(value.clone());
        assert_eq!(Value::from(content.clone()), value);
        assert_eq!(content.to_json_value(), value);
    }

    #[test]
    fn bytes_are_the_one_value_json_cannot_hold() {
        // The lossy direction, and the reason `Wire` exists: both of these are the same
        // JSON document, and only the format and the wire say which one it meant.
        let bytes = Content::Bytes(b"RIFF".to_vec());
        assert_eq!(bytes.to_json_value(), json!("UklGRg=="));
        assert_eq!(
            Content::from(bytes.to_json_value()),
            Content::String("UklGRg==".into())
        );
    }

    #[test]
    fn accessors_do_not_confuse_text_with_bytes() {
        let text = Content::String("UklGRg==".into());
        let bytes = Content::Bytes(b"RIFF".to_vec());
        assert_eq!(text.as_str(), Some("UklGRg=="));
        assert_eq!(text.as_bytes(), None);
        assert_eq!(bytes.as_bytes(), Some(&b"RIFF"[..]));
        assert_eq!(bytes.as_str(), None);
        assert!(text.is_string() && !text.is_bytes());
        assert!(bytes.is_bytes() && !bytes.is_string());
        assert!(Content::Null.is_null());
    }

    #[test]
    fn conversions_cover_the_types_a_caller_has_at_hand() {
        assert_eq!(Content::from("hi"), Content::String("hi".into()));
        assert_eq!(
            Content::from("hi".to_string()),
            Content::String("hi".into())
        );
        assert_eq!(Content::from(vec![1u8, 2]), Content::Bytes(vec![1, 2]));
        assert_eq!(Content::from(&b"ab"[..]), Content::Bytes(b"ab".to_vec()));
        assert_eq!(Content::from(true), Content::Bool(true));
        assert_eq!(Content::from(-1i64).to_json_value(), json!(-1));
        assert_eq!(Content::from(1u64).to_json_value(), json!(1));
        assert_eq!(Content::from(1.5f64).to_json_value(), json!(1.5));
        assert_eq!(
            Content::from(vec![Content::Null]),
            Content::Array(vec![Content::Null])
        );
        let fields = BTreeMap::from([("a".to_string(), Content::Bool(false))]);
        assert_eq!(Content::from(fields.clone()), Content::Object(fields));
    }

    #[test]
    fn a_non_finite_float_built_locally_becomes_null_rather_than_a_panic() {
        // ECMA-404 has no NaN and no infinity. `serde_json` resolves this the same way,
        // and a caller's arithmetic must not be able to abort the process. This is the
        // constructor, not the decoder — see the test below for the other direction.
        assert_eq!(Content::from(f64::NAN), Content::Null);
        assert_eq!(Content::from(f64::INFINITY), Content::Null);
        assert_eq!(Content::from(f64::NEG_INFINITY), Content::Null);
    }

    #[test]
    fn a_non_finite_float_on_the_wire_is_refused_rather_than_read_as_null() {
        // CBOR can send one (RFC 8949 §3.3) and ECMA-404 cannot hold it. Reading it as
        // null would hand the application a value the peer did not send — an infinite
        // sensor reading arriving as "no reading" — so the message does not decode, as
        // for an integer the number space cannot hold.
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let error = ContentVisitor
                .visit_f64::<ValueError>(value)
                .expect_err("a non-finite float is not ECMA-404 content");
            assert!(error.to_string().contains("outside the range"), "{error}");
        }
        assert_eq!(
            ContentVisitor.visit_f64::<ValueError>(1.5).unwrap(),
            Content::Number(Number::from_f64(1.5).unwrap())
        );
    }

    #[test]
    fn base64_encoding_is_padded_and_decoding_is_not_particular_about_it() {
        assert_eq!(base64_encode(b"RIFF"), "UklGRg==");
        assert_eq!(base64_decode("UklGRg==").unwrap(), b"RIFF");
        // §6.4 says "JSON Base64 encoding" and never says "padded", so an unpadded but
        // otherwise valid payload is accepted rather than rejected on a rule that is
        // not in the text.
        assert_eq!(base64_decode("UklGRg").unwrap(), b"RIFF");
    }

    #[test]
    fn the_url_safe_alphabet_is_a_different_encoding_and_is_rejected() {
        // Leniency about padding is not leniency about the alphabet: `-` and `_` mean
        // nothing in the encoding §6.4 asks for, and guessing would silently change the
        // payload's bytes.
        assert!(base64_decode("-_8=").is_err());
    }

    #[test]
    fn an_integer_no_ecma_404_number_can_hold_is_rejected_rather_than_rounded() {
        // CBOR reaches further than JSON does (RFC 8949 §3.1, and bignums further
        // still). Rounding into an f64 would change the peer's value without saying so.
        let error = ContentVisitor
            .visit_i128::<ValueError>(i128::from(u64::MAX) + 1)
            .expect_err("2^64 is outside the ECMA-404 number space");
        assert!(error.to_string().contains("outside the range"), "{error}");

        // Inside the range it is a number like any other, whichever side of zero.
        assert_eq!(
            ContentVisitor
                .visit_i128::<ValueError>(i128::from(u64::MAX))
                .unwrap(),
            Content::Number(u64::MAX.into())
        );
        assert_eq!(
            ContentVisitor.visit_i128::<ValueError>(-1).unwrap(),
            Content::Number((-1i64).into())
        );
    }

    #[test]
    fn an_object_key_that_is_not_text_is_outside_nlip() {
        // A CBOR map may be keyed by anything (RFC 8949 §3.1); an ECMA-404 object may
        // not, and §5 makes an NLIP message an ECMA-404 object.
        let map = MapDeserializer::new(vec![(1u32, "x")].into_iter());
        let error: ValueError = Content::deserialize(map).expect_err("integer key");
        assert!(error.to_string().contains("text string"), "{error}");
    }

    #[test]
    fn a_text_key_is_accepted_from_any_codec() {
        let map = MapDeserializer::new(vec![("a", 1u8)].into_iter());
        let content: Content = Content::deserialize(map)
            .map_err(|error: ValueError| error)
            .expect("text key");
        assert_eq!(content.to_json_value(), json!({"a": 1}));
    }

    #[test]
    fn an_optional_value_decodes_to_its_contents_or_to_null() {
        // `visit_some` / `visit_none` / `visit_unit`, which a codec reaches for an
        // absent or optional value. No deserializer in `serde::de::value` produces
        // them, so the visitor is driven directly.
        let inner: StrDeserializer<ValueError> = "hi".into_deserializer();
        assert_eq!(
            ContentVisitor.visit_some(inner).unwrap(),
            Content::String("hi".into())
        );
        assert_eq!(
            ContentVisitor.visit_none::<ValueError>().unwrap(),
            Content::Null
        );
        assert_eq!(
            ContentVisitor.visit_unit::<ValueError>().unwrap(),
            Content::Null
        );
    }

    #[test]
    fn json_serialisation_spells_bytes_as_base64() {
        // serde_json is human-readable, so `Serialize` picks the §6.4 spelling without
        // the caller choosing it.
        let content = Content::Array(vec![Content::Bytes(b"RIFF".to_vec())]);
        assert_eq!(serde_json::to_string(&content).unwrap(), r#"["UklGRg=="]"#);
    }
}
