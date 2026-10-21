//! The language-neutral conformance corpus, as a harness any member can run.
//!
//! The cases live at `conformance/` in the repository root, outside every package, and
//! are data rather than Rust: a recorded document and the outcome every implementation
//! must produce for it. That is the point of them. A test that builds its fixture out
//! of this workspace's own encoder agrees with a systematic misreading of clause 5 just
//! as readily as with a correct one, whereas a case copied out of ECMA-430 Annex A,
//! TR/113 §11 or ECMA-432 §8 does not — and the same files can be handed to a second
//! implementation in another language, which is what the corpus exists for.
//!
//! `conformance/README.md` defines the case formats, the error-kind and rule
//! vocabularies, and the canonical spelling of a decoded message. This crate is the
//! Rust harness for them: it maps [`ParseError`] onto the corpus's kinds ([`describe`]),
//! projects a [`Message`] into the corpus's canonical model ([`canonical_model`]),
//! compares one case ([`run_case`], [`run_exchange_case`]), and walks a whole directory
//! of them ([`run`]).
//!
//! # Two kinds of case
//!
//! A **message** case is bytes in, decode-and-validate out: it can say everything about
//! what a message *is* (ECMA-430 clause 5) and nothing about what an end-point must
//! *do* with one. A **exchange** case is the second kind, in [`exchange`]: a request, a
//! scripted end-point, and the properties clause 6 obliges its reply to have. The two
//! live in one tree and [`run`] walks both; a file's `kind` field says which it is, and
//! an absent one means `message`, because the message cases came first and every one of
//! them predates the key.
//!
//! # Why this is a crate, and an unpublished one
//!
//! The corpus sits above the crate under test, so it cannot travel inside a `.crate`
//! tarball; a runner shipped without its cases is a test that fails for whoever builds
//! from the tarball. As a test file inside `nlipsy-protocol` it had to be excluded from
//! the package by hand, and it made the library carry a `serde` `derive` dev-dependency
//! the library itself never uses. An unpublished workspace member removes both — the
//! arrangement jsonrpsee uses for its integration tests.
//!
//! Being a library rather than a test file is what lets a second member run the same
//! cases: an ECMA-431 or ECMA-432 binding that transcodes a message has to prove it
//! preserved it, and the proof is this corpus applied to what came out the far side.
//!
//! # Reporting
//!
//! Every case runs even when an earlier one has failed, and each difference is one line
//! naming the file: a corpus that stopped at the first mismatch would turn a reading of
//! the standard into one bisection per case.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use nlipsy_protocol::{Content, Message, ParseError, Submessage};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

pub mod exchange;

#[cfg(feature = "component")]
pub mod component;

pub use exchange::{ExchangeCase, run_exchange_case};

// ---------------------------------------------------------------------------
// The case format — conformance/README.md
//
// `deny_unknown_fields` on purpose: a key the harness does not know is a key the
// harness is silently not checking, which is the one failure mode a corpus cannot
// afford. A new expectation must therefore be added here before it can be added to
// a case, and a typo in a case file fails loudly instead of passing quietly. In
// `Case` the leftover keys go to the flattened `Prose`, so it is that struct's own
// `deny_unknown_fields` that refuses them.
// ---------------------------------------------------------------------------

/// One case file: a recorded document and the outcome it must produce.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// `message`, or absent, which means the same.
    ///
    /// The key exists because the corpus grew a second kind (`exchange`, in
    /// [`exchange`]) after every case here was already written.
    /// Defaulting it rather than adding it to ninety-seven files keeps those files
    /// evidence about the protocol and not about a schema migration, and a case that
    /// does spell it is accepted so that a new one can say what it is.
    #[serde(default = "message_kind")]
    pub kind: String,
    /// `json` or `cbor` — which decoder the input is handed to.
    pub wire: String,
    /// A JSON wire's document, as text: casing, duplicate keys and whitespace are all
    /// things the corpus has cases about, and a parsed value would have discarded them.
    pub input: Option<String>,
    /// A CBOR wire's frame, in lower-case hex, for the same reason: map order and
    /// definite-length heads are part of what was recorded.
    pub input_hex: Option<String>,
    /// What decoding the input must produce.
    pub expect: Expect,
    /// Declared so that `deny_unknown_fields` accepts them, and so that a case cannot
    /// be written without them. They are addressed to a reader — what the case is,
    /// where the document came from, and which doubt it carries — and the harness has
    /// nothing to compare them against.
    #[serde(flatten)]
    pub prose: Prose,
}

/// The parts of a case addressed to a reader rather than to the harness.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prose {
    /// One line, what the case is about.
    pub title: String,
    /// Where the recorded document came from.
    pub source: Source,
    /// A doubt about the expectation, a conflict with the standard, or a defensible
    /// alternative reading.
    pub notes: Option<String>,
}

/// Provenance: a fixture copied from the standard is evidence about the protocol, and a
/// fixture someone wrote is evidence about an implementation's opinion.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// `ECMA-430`, `ECMA-432`, `Ecma TR/113` or `ECMA-404`.
    pub document: String,
    /// The clause, figure or example number the input comes from.
    pub clause: String,
    /// Begins `verbatim`, `verbatim, minimally repaired` or `composed`.
    pub note: String,
}

/// The expected outcome of decoding a case's input.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// `ok` or `error`.
    pub decode: String,
    /// The single failure expected, when `decode` is `error`.
    pub error: Option<ExpectedError>,
    /// The decoded message in the corpus's canonical model spelling.
    pub message: Option<Value>,
    /// Every finding `validate` must report, compared as a set.
    pub violations: Option<Vec<ExpectedViolation>>,
    /// Whether §5.1.1's reserved `messagetype` or §6.3's boolean field makes this a
    /// control message.
    pub is_control: Option<bool>,
    /// All text content joined by single spaces, in document order (§5.1.5).
    ///
    /// Two layers of [`Option`], because the two absences are different: the outer one
    /// is "the case file has no `text` key", which is a malformed case, and the inner
    /// one is an explicit `null`, which is "this message carries no text at all".
    /// serde reads a JSON `null` into an `Option` as `None`, so telling them apart
    /// needs a deserializer of this crate's own rather than the derive's default.
    #[serde(default, deserialize_with = "present")]
    pub text: Option<Option<String>>,
    /// The token submessages, classified — optional, like the part of an implementation
    /// it tests.
    pub tokens: Option<Vec<ExpectedToken>>,
    /// How many `token` submessages the message carries, the **first submessage
    /// included** — `nlipsy_core::token_count`.
    ///
    /// Deliberately not derivable from [`Self::tokens`], which is the whole reason the
    /// key exists: that list is the `submessages` array and this number is the array
    /// plus the first submessage, so the two disagree by one on exactly the message
    /// §6.2 is easiest to get wrong about. It is also what the token ceiling counts,
    /// which is why an implementation that derived it from the list would refuse the
    /// wrong requests.
    pub token_count: Option<u32>,
    /// The first submessage as a `token` submessage, when it is one —
    /// `nlipsy_core::first_submessage_token`.
    ///
    /// Two layers of [`Option`] for the reason [`Self::text`] has them: the outer is
    /// "the case says nothing", the inner is an explicit `null`, which asserts the first
    /// submessage is **not** a token. A message case's `message` cannot say this on its
    /// own — the model spells the first submessage as the message's own `format`,
    /// `subformat` and `content`, and §5 ¶2's reading that this *is* a submessage for
    /// §6.2's purposes is a rule rather than a projection.
    #[serde(default, deserialize_with = "present")]
    pub first_submessage_token: Option<Option<ExpectedFirstSubmessageToken>>,
    /// The exact string a re-encoder must produce — optional, for the same reason.
    pub canonical_json: Option<String>,
}

/// A failure, in the corpus's vocabulary of kinds.
///
/// An implementation is not expected to reproduce the *message* of an error, only its
/// kind, path and field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedError {
    /// `invalid-json`, `cbor`, `not-object`, `missing-field`, `wrong-type`,
    /// `duplicate-field`, `unknown-format` or `base64`.
    pub kind: String,
    /// `$` for the document or its first submessage, `submessages[i]` otherwise.
    pub path: String,
    /// The canonical lower-case field name, for the kinds that name one.
    pub field: Option<String>,
}

/// One finding of `validate`, as `{rule, severity, path}`.
#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct ExpectedViolation {
    /// The stable rule id, so a finding can be asserted without matching prose.
    pub rule: String,
    /// `error` — the message cannot be interpreted as written — or `warning`.
    pub severity: String,
    /// Where in the message the finding is about.
    pub path: String,
}

/// The first submessage, read as §6.2 reads it: a `token` submessage of its own.
///
/// `format` is not a field here, because there is only one value it could hold: the
/// predicate is "the first submessage is a token", and a case spelling `token` back
/// would be asserting the question rather than the answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedFirstSubmessageToken {
    /// The subformat as it arrived, casing included — §6.2 ¶2's "exact same".
    pub subformat: String,
    /// The content in the canonical model spelling.
    pub content: Value,
    /// The label, when the case is about one. Absent asserts nothing, an explicit
    /// `null` asserts there is none. ECMA-430 defines no label for a first submessage,
    /// so whether one travels with the copy is a reading — `readings/41` is where it is
    /// recorded.
    #[serde(default, deserialize_with = "present")]
    pub label: Option<Option<String>>,
}

/// One token submessage of the `submessages` array, classified.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedToken {
    /// The subformat as it arrived, casing included.
    pub subformat: String,
    /// The token's content in the canonical model spelling.
    pub content: Value,
    /// `conversation`, `authentication` or `other` (§5.3, §6.2).
    pub kind: String,
}

fn message_kind() -> String {
    "message".to_owned()
}

/// Deserialize a field that is allowed to be `null`, keeping "absent" distinct.
///
/// serde's own handling of `Option` collapses the two — a missing key and a `null` both
/// arrive as `None` — which is right for a field that is simply optional and wrong for
/// an expectation, where a missing key means the case forgot to say anything.
pub(crate) fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

// ---------------------------------------------------------------------------
// This implementation, as the corpus spells it
// ---------------------------------------------------------------------------

/// A [`ParseError`] in the corpus's fixed vocabulary — what a second implementation in
/// another language would have to report for the same bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDescription {
    /// The corpus's kind, or `unmapped` for a [`ParseError`] variant added since.
    pub kind: &'static str,
    /// The place the error names, as `ParseError::path` spells it.
    pub path: String,
    /// The field the kind is about, where the kind names one.
    pub field: Option<String>,
}

/// Describe a [`ParseError`] as the corpus's kind, path and field.
///
/// The *kind* is [`ParseError::kind`] — the mapping is the protocol crate's, because it
/// is what this implementation claims to have distinguished and the JavaScript package's
/// component has to report the same nine values for the same bytes. It used to be here,
/// with a `_` arm answering `unmapped` for a variant this harness did not know; a
/// variant added now stops `ParseError::kind` compiling instead, which is the stronger
/// check and the reason it moved.
///
/// The *field* stays here. Two variants carry none of their own and one is supplied,
/// because it is the only field the variant can be about: `UnknownFormat` is always
/// about `format`, and `Base64` always about `content` (§6.4 spells only a `binary`
/// content in base64).
pub fn describe(error: &ParseError) -> ErrorDescription {
    let field = match error {
        ParseError::MissingField { field, .. } => Some((*field).to_owned()),
        ParseError::WrongType { field, .. } => Some(field.to_string()),
        ParseError::DuplicateField { field, .. } => Some(field.clone()),
        ParseError::UnknownFormat { .. } => Some("format".to_owned()),
        ParseError::Base64 { .. } => Some("content".to_owned()),
        // `ParseError` is `#[non_exhaustive]`: a variant added since names no field
        // here, which is what the corpus asserts for every kind that has none.
        _ => None,
    };
    ErrorDescription {
        kind: error.kind().as_str(),
        path: error.path().to_owned(),
        field,
    }
}

/// Project a content value into the corpus's canonical model spelling.
///
/// The only difference from `Content::to_json_value` is `Content::Bytes`: JSON spells it
/// as base64 (§6.4), which is indistinguishable from a string that happens to look like
/// base64, so the model wraps it in `{"$bytes": …}`. The base64 itself is still the
/// crate's own — the corpus asserts the padded standard spelling, and taking it from
/// anywhere else would be testing a second encoder.
pub fn canonical_content(content: &Content) -> Value {
    match content {
        Content::Bytes(_) => json!({ "$bytes": content.to_json_value() }),
        Content::Array(items) => Value::Array(items.iter().map(canonical_content).collect()),
        Content::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), canonical_content(value)))
                .collect::<Map<String, Value>>(),
        ),
        other => other.to_json_value(),
    }
}

/// Project a decoded message into the corpus's canonical model spelling.
///
/// Absent optional fields are omitted rather than spelled `null`, because `content:
/// null` is a value Annex A allows and an absent field is not the same thing.
pub fn canonical_model(message: &Message) -> Value {
    let mut out = Map::new();
    if let Some(message_type) = &message.message_type {
        out.insert("message_type".into(), json!(message_type));
    }
    if let Some(control) = message.control {
        out.insert("control".into(), json!(control));
    }
    if let Some(label) = &message.label {
        out.insert("label".into(), json!(label));
    }
    out.insert("format".into(), json!(message.format.as_str()));
    out.insert("subformat".into(), json!(message.subformat));
    out.insert("content".into(), canonical_content(&message.content));
    if !message.submessages.is_empty() {
        out.insert(
            "submessages".into(),
            Value::Array(
                message
                    .submessages
                    .iter()
                    .map(canonical_submessage)
                    .collect(),
            ),
        );
    }
    Value::Object(out)
}

fn canonical_submessage(submessage: &Submessage) -> Value {
    let mut out = Map::new();
    if let Some(label) = &submessage.label {
        out.insert("label".into(), json!(label));
    }
    out.insert("format".into(), json!(submessage.format.as_str()));
    out.insert("subformat".into(), json!(submessage.subformat));
    out.insert("content".into(), canonical_content(&submessage.content));
    Value::Object(out)
}

/// The token submessages, classified as §5.3 and §6.2 classify them.
///
/// Only the two reserved prefixes are recognised; every other subformat is the
/// application's own, which is what `other` means.
pub fn canonical_tokens(message: &Message) -> Vec<Value> {
    message
        .tokens()
        .map(|token| {
            let kind = if token.is_conversation_token() {
                "conversation"
            } else if token.is_authentication_token() {
                "authentication"
            } else {
                "other"
            };
            json!({
                "subformat": token.subformat,
                "content": canonical_content(&token.content),
                "kind": kind,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// What an implementation produced
// ---------------------------------------------------------------------------

/// Everything a **message** case asks of an implementation, as that implementation
/// answered it.
///
/// The expectations are compared against *this* and never against a [`Message`], and
/// that is what lets a second implementation be measured by the same code: the Rust
/// workspace fills one in through [`Observed::of`], and the `component` mode fills one
/// in by calling the built component's exports. A comparison written against a
/// `Message` would have re-run this workspace's own rules and reported them as the
/// other implementation's.
#[derive(Debug)]
pub struct Observed {
    /// The decoded message in the corpus's canonical model spelling.
    pub model: Value,
    /// Every finding `validate` reported, as `{rule, severity, path}`.
    pub violations: Vec<ExpectedViolation>,
    /// §5.1.1's reserved value or §6.3's boolean.
    pub is_control: bool,
    /// All text content joined by single spaces, in document order.
    pub text: Option<String>,
    /// The token submessages, classified.
    pub tokens: Vec<Value>,
    /// How many `token` submessages the message carries, the first submessage included.
    pub token_count: u32,
    /// The first submessage as a `token` submessage, when it is one.
    pub first_submessage_token: Option<ObservedToken>,
    /// The bytes a re-encoder produced on the JSON wire.
    pub canonical_json: String,
}

/// One token submessage as an implementation handed it back.
///
/// Its own type rather than a [`Value`], because the `label` is compared three ways —
/// asserted present, asserted absent, or not asserted at all — and a JSON object has no
/// spelling for the third.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedToken {
    /// The subformat, byte for byte.
    pub subformat: String,
    /// The content in the canonical model spelling.
    pub content: Value,
    /// The label, when there is one.
    pub label: Option<String>,
}

impl ObservedToken {
    /// One token as a comparable line, so a difference reads as text.
    pub fn line(&self) -> String {
        let label = match &self.label {
            Some(label) => format!(" label={label}"),
            None => String::new(),
        };
        format!(
            "token/{} = {}{label}",
            self.subformat,
            compact(&self.content)
        )
    }
}

impl Observed {
    /// What this workspace produces for one decoded message.
    pub fn of(message: &Message) -> Observed {
        Observed {
            model: canonical_model(message),
            violations: message
                .validate()
                .iter()
                .map(|violation| ExpectedViolation {
                    rule: violation.rule.to_owned(),
                    severity: violation.severity.to_string(),
                    path: violation.path.clone(),
                })
                .collect(),
            is_control: message.is_control(),
            text: message.extract_text(None),
            tokens: canonical_tokens(message),
            // The core's, not a count taken here: it is the number the token ceiling is
            // applied to, and a corpus that recounted it would be asserting its own
            // arithmetic rather than the implementation's.
            token_count: u32::try_from(nlipsy_core::token_count(message)).unwrap_or(u32::MAX),
            first_submessage_token: nlipsy_core::first_submessage_token(message)
                .as_ref()
                .map(observed_token),
            canonical_json: message.to_json_string(),
        }
    }
}

/// One [`Submessage`] as the corpus compares one.
pub fn observed_token(token: &Submessage) -> ObservedToken {
    ObservedToken {
        subformat: token.subformat.clone(),
        content: canonical_content(&token.content),
        label: token.label.clone(),
    }
}

/// Which implementation the corpus is being run against.
///
/// One method per case kind, because the two kinds ask different things: a message case
/// is bytes in, decode-and-validate out, and an exchange case is clause 6's two
/// synchronous steps over values. [`run_with`] walks the tree, groups the files and
/// writes the report; this is the only part that differs between the Rust workspace and
/// the component in wasmtime.
pub trait Implementation {
    /// Run one message case.
    fn message_case(&mut self, case: &Case) -> Outcome;
    /// Run one exchange case.
    fn exchange_case(&mut self, case: &ExchangeCase) -> Outcome;
}

/// This workspace, as an [`Implementation`].
#[derive(Debug, Default)]
pub struct Workspace;

impl Implementation for Workspace {
    fn message_case(&mut self, case: &Case) -> Outcome {
        run_case(case)
    }

    fn exchange_case(&mut self, case: &ExchangeCase) -> Outcome {
        run_exchange_case(case)
    }
}

// ---------------------------------------------------------------------------
// Running one case
// ---------------------------------------------------------------------------

/// What one case produced that it should not have. Empty means it passed.
pub(crate) type Differences = Vec<String>;

/// Decode a CBOR frame, or `None` when this build has no CBOR codec.
///
/// ECMA-432 §7.1 is behind the `cbor` feature, which is off by default, so a default
/// build cannot run those cases at all. They are skipped and counted rather than
/// quietly passed: a corpus that reported nothing for the cases it never ran would say
/// the implementation conforms on a wire it cannot speak.
#[cfg(feature = "cbor")]
fn decode_cbor(bytes: &[u8]) -> Option<Result<Message, ParseError>> {
    Some(Message::from_cbor_slice(bytes))
}

#[cfg(not(feature = "cbor"))]
fn decode_cbor(_bytes: &[u8]) -> Option<Result<Message, ParseError>> {
    None
}

/// The outcome of one case file.
#[derive(Debug)]
pub enum Outcome {
    /// Everything the case expects is what the implementation produced.
    Passed,
    /// A CBOR case in a build without the `cbor` feature. Counted, never passed.
    Skipped,
    /// One line per difference, each naming what was expected and what arrived.
    Failed(Vec<String>),
}

/// Run one case against this workspace's implementation.
pub fn run_case(case: &Case) -> Outcome {
    let decoded = match case.wire.as_str() {
        "json" => {
            let Some(input) = &case.input else {
                return Outcome::Failed(vec!["a json case without `input`".into()]);
            };
            Message::from_json_bytes(input.as_bytes())
        }
        "cbor" => {
            let Some(hex) = &case.input_hex else {
                return Outcome::Failed(vec!["a cbor case without `input_hex`".into()]);
            };
            let bytes = match from_hex(hex) {
                Ok(bytes) => bytes,
                Err(why) => return Outcome::Failed(vec![why]),
            };
            match decode_cbor(&bytes) {
                Some(decoded) => decoded,
                None => return Outcome::Skipped,
            }
        }
        other => return Outcome::Failed(vec![format!("unknown wire `{other}`")]),
    };

    let differences = match (case.expect.decode.as_str(), decoded) {
        ("error", Err(error)) => compare_description(case, &describe(&error)),
        ("error", Ok(message)) => vec![format!(
            "decode: expected an error, got a message ({})",
            message.to_json_string()
        )],
        ("ok", Err(error)) => {
            let described = describe(&error);
            vec![format!(
                "decode: expected a message, got {} at {}: {error}",
                described.kind, described.path
            )]
        }
        ("ok", Ok(message)) => compare_observed(case, &Observed::of(&message)),
        (other, _) => vec![format!("unknown expect.decode `{other}`")],
    };

    if differences.is_empty() {
        Outcome::Passed
    } else {
        Outcome::Failed(differences)
    }
}

/// Compare one implementation's description of a decode failure with the case's.
///
/// An implementation is not expected to reproduce the *message* of an error, only its
/// kind, path and field — which is exactly what [`ErrorDescription`] carries, and why
/// this takes one rather than a [`ParseError`]: the component reports the same three
/// fields across a boundary that has no Rust error type on it.
pub fn compare_description(case: &Case, described: &ErrorDescription) -> Differences {
    let Some(expected) = &case.expect.error else {
        return vec!["expect.decode is `error` with no `error` object".into()];
    };
    let mut out = Vec::new();
    if described.kind != expected.kind {
        out.push(format!(
            "error.kind: expected `{}`, got `{}`",
            expected.kind, described.kind
        ));
    }
    if described.path != expected.path {
        out.push(format!(
            "error.path: expected `{}`, got `{}`",
            expected.path, described.path
        ));
    }
    if described.field != expected.field {
        out.push(format!(
            "error.field: expected {:?}, got {:?}",
            expected.field, described.field
        ));
    }
    out
}

/// Compare what an implementation produced for one message case with what the case
/// expects.
///
/// Every expectation of a message case is here and nowhere else, so that the Rust
/// workspace and the component are held to the same reading of the same file.
pub fn compare_observed(case: &Case, observed: &Observed) -> Differences {
    let mut out = Vec::new();

    match &case.expect.message {
        Some(expected) => {
            if &observed.model != expected {
                out.push(format!(
                    "message: expected {}, got {}",
                    compact(expected),
                    compact(&observed.model)
                ));
            }
        }
        None => out.push("expect.decode is `ok` with no `message`".into()),
    }

    match &case.expect.violations {
        Some(expected) => {
            // §5.3's findings have no order the standard gives them, so they are
            // compared as a set: an implementation that reports the submessages first
            // is not thereby wrong.
            let expected: BTreeSet<&ExpectedViolation> = expected.iter().collect();
            let actual: BTreeSet<&ExpectedViolation> = observed.violations.iter().collect();
            for missing in expected.difference(&actual) {
                out.push(format!(
                    "violations: expected but not reported: {missing:?}"
                ));
            }
            for extra in actual.difference(&expected) {
                out.push(format!("violations: reported but not expected: {extra:?}"));
            }
        }
        None => out.push("expect.decode is `ok` with no `violations`".into()),
    }

    match case.expect.is_control {
        Some(expected) if observed.is_control != expected => {
            out.push(format!(
                "is_control: expected {expected}, got {}",
                observed.is_control
            ));
        }
        Some(_) => {}
        None => out.push("expect.decode is `ok` with no `is_control`".into()),
    }

    match &case.expect.text {
        Some(expected) => {
            if observed.text.as_deref() != expected.as_deref() {
                out.push(format!(
                    "text: expected {expected:?}, got {:?}",
                    observed.text
                ));
            }
        }
        None => out.push("expect.decode is `ok` with no `text`".into()),
    }

    if let Some(expected) = &case.expect.tokens {
        let expected = expected
            .iter()
            .map(|token| {
                json!({
                    "subformat": token.subformat,
                    "content": token.content,
                    "kind": token.kind,
                })
            })
            .collect::<Vec<_>>();
        if observed.tokens != expected {
            out.push(format!(
                "tokens: expected {}, got {}",
                compact(&Value::Array(expected)),
                compact(&Value::Array(observed.tokens.clone()))
            ));
        }
    }

    if let Some(expected) = case.expect.token_count {
        if observed.token_count != expected {
            out.push(format!(
                "token_count: expected {expected}, got {}",
                observed.token_count
            ));
        }
    }

    if let Some(expected) = &case.expect.first_submessage_token {
        match (expected, &observed.first_submessage_token) {
            (None, None) => {}
            (None, Some(actual)) => out.push(format!(
                "first_submessage_token: expected none, got {}",
                actual.line()
            )),
            (Some(expected), None) => out.push(format!(
                "first_submessage_token: expected token/{}, got none",
                expected.subformat
            )),
            (Some(expected), Some(actual)) => {
                // An absent `label` key asserts nothing about one, which is the same
                // rule an exchange case's `expect.reply.tokens[].label` follows.
                let label_matches = match &expected.label {
                    None => true,
                    Some(wanted) => actual.label.as_deref() == wanted.as_deref(),
                };
                if !label_matches
                    || actual.subformat != expected.subformat
                    || actual.content != expected.content
                {
                    out.push(format!(
                        "first_submessage_token: expected token/{} = {}, got {}",
                        expected.subformat,
                        compact(&expected.content),
                        actual.line()
                    ));
                }
            }
        }
    }

    if let Some(expected) = &case.expect.canonical_json {
        if &observed.canonical_json != expected {
            out.push(format!(
                "canonical_json: expected {expected}, got {}",
                observed.canonical_json
            ));
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Walking the corpus
// ---------------------------------------------------------------------------

/// One thing a case produced that it should not have, and which case produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// The case file, relative to the directory [`run`] was given, so that a failure
    /// says which case before it says what about it.
    pub case: String,
    /// `message`, `exchange`, or `unknown` for a file whose `kind` is neither.
    ///
    /// Two kinds of case fail for different reasons — a message case for what its bytes
    /// decoded to, an exchange case for what clause 6 did with them — and the kind is
    /// on the line so that a reader knows which question the difference is an answer to
    /// before opening the file.
    pub kind: &'static str,
    /// One line: what was expected, and what arrived.
    pub detail: String,
}

/// What one group of the corpus found. One group is one directory under `cases/`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GroupReport {
    /// Case files found in the group.
    pub cases: usize,
    /// Cases whose every expectation was met.
    pub passed: usize,
    /// CBOR cases skipped for want of the `cbor` feature.
    pub skipped: usize,
    /// Cases with at least one difference.
    pub failed: usize,
}

/// What a run of the corpus found.
///
/// The counts and the differences are both part of the answer: `failed == 0` with
/// `cases == 0` is a corpus that was never found, which is the one way a run can be
/// empty and look like success. The caller checks `cases` as well — a missing directory
/// leaves a [`Difference`] here saying so, but a caller that only looked at
/// `differences` would still be told the truth by `cases`.
#[derive(Debug, Default)]
pub struct Report {
    /// Case files found, whether or not they could be read.
    pub cases: usize,
    /// Of those, the ones whose `kind` is `message` (or absent).
    pub message_cases: usize,
    /// Of those, the ones whose `kind` is `exchange`.
    pub exchange_cases: usize,
    /// Cases whose every expectation was met.
    pub passed: usize,
    /// CBOR cases skipped because this build has no `cbor` feature (ECMA-432 §7.1).
    pub skipped: usize,
    /// Cases with at least one difference, or that could not be read at all.
    pub failed: usize,
    /// The same counts per group, keyed by the directory under `cases/`.
    ///
    /// A total says whether the corpus passed; the breakdown says what it covers. Which
    /// is worth knowing per run, because the groups answer different questions — what
    /// the standard says (`ecma-430`), what a binding says (`ecma-432`), what the
    /// explanatory report shows (`tr-113`), and what this implementation decided where
    /// the text is silent (`readings`).
    pub groups: BTreeMap<String, GroupReport>,
    /// Every difference, in case order: one line each, so a failing run is a list of
    /// facts rather than a bisection.
    pub differences: Vec<Difference>,
}

impl fmt::Display for Report {
    /// The differences first, then the per-group counts, then the totals — a summary
    /// belongs after what it summarises.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for difference in &self.differences {
            writeln!(
                f,
                "{} [{}]: {}",
                difference.case, difference.kind, difference.detail
            )?;
        }
        for (group, counts) in &self.groups {
            write!(f, "conformance: {group}: {} passed", counts.passed)?;
            if counts.skipped > 0 {
                write!(f, ", {} skipped", counts.skipped)?;
            }
            if counts.failed > 0 {
                write!(f, ", {} failed", counts.failed)?;
            }
            writeln!(f, ", of {}", counts.cases)?;
        }
        write!(
            f,
            "conformance: {} passed, {} skipped, {} failed, of {} cases ({} message, {} exchange)",
            self.passed,
            self.skipped,
            self.failed,
            self.cases,
            self.message_cases,
            self.exchange_cases
        )?;
        if self.skipped > 0 {
            write!(
                f,
                "\nconformance: {} CBOR cases need `--features cbor` (ECMA-432 §7.1)",
                self.skipped
            )?;
        }
        Ok(())
    }
}

/// Run every case under `cases`, and report what they found.
///
/// `cases` is the corpus's `cases/` directory — `conformance/cases` in this repository,
/// which sits above every package rather than inside one. One corpus for every
/// implementation is the whole point; a copy under a crate's `tests/` would be a second
/// corpus that agrees with that crate by construction.
///
/// Both kinds of case are walked from this one tree. A file's `kind` field decides which
/// runner it goes to, and a file without one is a message case — every case written
/// before the second kind existed is in that state, and migrating ninety-seven files to
/// spell a default would have edited evidence to satisfy a schema.
///
/// Nothing here panics and nothing stops early: a directory that cannot be read, a file
/// that cannot be read, a file that is not a well-formed case and a case whose
/// expectations are not met all become differences in the [`Report`], and the run
/// continues.
pub fn run(cases: &Path) -> Report {
    run_with(cases, &mut Workspace)
}

/// Run every case under `cases` against `implementation`, and report what they found.
///
/// [`run`] is this with the Rust workspace as the implementation; the `component` mode
/// of this crate passes a host that calls the built component's exports instead. The
/// walk, the grouping, the counts and the summary lines are here and are therefore the
/// same for both — a second runner with its own report would be a second corpus.
pub fn run_with(cases: &Path, implementation: &mut dyn Implementation) -> Report {
    let mut report = Report::default();
    let files = case_files(cases, &mut report);
    report.cases = files.len();

    for file in &files {
        let relative = file.strip_prefix(cases).unwrap_or(file);
        let name = relative.display().to_string();
        let group = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| "(root)".to_owned());
        report.groups.entry(group.clone()).or_default().cases += 1;

        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(why) => {
                report.record(
                    &group,
                    &name,
                    "unknown",
                    Outcome::Failed(vec![format!("cannot be read: {why}")]),
                );
                continue;
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(why) => {
                report.record(
                    &group,
                    &name,
                    "unknown",
                    Outcome::Failed(vec![format!("not JSON at all: {why}")]),
                );
                continue;
            }
        };

        // The `kind` probe reads the raw value rather than a struct, because the two
        // kinds are different types with `deny_unknown_fields` on each: deserializing
        // into the wrong one would fail on the *other* kind's keys and report a schema
        // error where the real answer is "this is the other kind of case".
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("message")
            .to_owned();
        let outcome = match kind.as_str() {
            "message" => {
                report.message_cases += 1;
                match serde_json::from_value::<Case>(value) {
                    Ok(case) => implementation.message_case(&case),
                    Err(why) => {
                        Outcome::Failed(vec![format!("not a well-formed message case: {why}")])
                    }
                }
            }
            "exchange" => {
                report.exchange_cases += 1;
                match serde_json::from_value::<ExchangeCase>(value) {
                    Ok(case) => implementation.exchange_case(&case),
                    Err(why) => {
                        Outcome::Failed(vec![format!("not a well-formed exchange case: {why}")])
                    }
                }
            }
            other => Outcome::Failed(vec![format!(
                "unknown case kind `{other}`; expected `message` or `exchange`"
            )]),
        };
        let kind = match kind.as_str() {
            "message" => "message",
            "exchange" => "exchange",
            _ => "unknown",
        };
        report.record(&group, &name, kind, outcome);
    }

    report
}

impl Report {
    /// Record one case's outcome, in the totals and in its group.
    fn record(&mut self, group: &str, case: &str, kind: &'static str, outcome: Outcome) {
        let counts = self.groups.entry(group.to_owned()).or_default();
        match outcome {
            Outcome::Passed => {
                counts.passed += 1;
                self.passed += 1;
            }
            Outcome::Skipped => {
                counts.skipped += 1;
                self.skipped += 1;
            }
            Outcome::Failed(differences) => {
                counts.failed += 1;
                self.failed += 1;
                for detail in differences {
                    self.differences.push(Difference {
                        case: case.to_owned(),
                        kind,
                        detail,
                    });
                }
            }
        }
    }

    /// Record a directory that could not be walked — one difference, and one more
    /// failure. It has no group of its own, because the thing that failed is where a
    /// group would have come from.
    fn fail(&mut self, case: &str, detail: String) {
        self.failed += 1;
        self.differences.push(Difference {
            case: case.to_owned(),
            kind: "unknown",
            detail,
        });
    }
}

/// Every `*.json` under `root`, sorted, so that a failure list reads the same twice.
///
/// A directory that cannot be read is a difference rather than a panic: the caller's
/// zero-case check is what turns a missing corpus into a failure, and a panic here would
/// hide every case the rest of the tree still holds.
fn case_files(root: &Path, report: &mut Report) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(why) => {
                report.fail(
                    &directory.display().to_string(),
                    format!("cannot be read: {why}"),
                );
                continue;
            }
        };
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(why) => {
                    report.fail(
                        &directory.display().to_string(),
                        format!("unreadable directory entry: {why}"),
                    );
                    continue;
                }
            };
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

pub(crate) fn from_hex(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err(format!("input_hex has an odd length ({})", text.len()));
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|why| format!("input_hex is not hex at byte {}: {why}", index / 2))
        })
        .collect()
}

/// A `Value` on one line, so that a difference is one line of output.
pub(crate) fn compact(value: &Value) -> String {
    serde_json::to_string(value).expect("a Value always serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A corpus that is not there must not read as a clean run.
    ///
    /// This is the one failure mode a report can have that looks like success: no cases
    /// found, so no differences, so nothing to fail on. `cases` is what a caller checks,
    /// and the directory is named in the report so the check does not have to guess why.
    #[test]
    fn a_missing_corpus_reports_itself_rather_than_passing() {
        let absent = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/no-such-corpus"));
        let report = run(absent);

        assert_eq!(report.cases, 0);
        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(report.differences.len(), 1);
        assert!(report.differences[0].case.ends_with("no-such-corpus"));
        assert!(report.to_string().contains("cannot be read"));
    }
}
