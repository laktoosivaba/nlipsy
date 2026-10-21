//! Failures of the message model — ECMA-430 clause 5.
//!
//! Both types here describe a *message*, not an endpoint: what was wrong with the
//! bytes, or which rule of clause 5 they broke. Nothing in this module knows that an
//! HTTP binding, a session table or a model exists, which is what lets a client and a
//! server share it without either one dragging the other's world along.
//!
//! The two are separated because they are handled differently. A [`ParseError`] means
//! the message cannot be interpreted at all — there is nothing to answer. A
//! [`Violation`] means it was understood but is off-spec, and a deployment may choose
//! to reject it or merely to log it (see [`Severity`]).

use thiserror::Error;

/// Structural failure while decoding an NLIP message.
///
/// Every variant names the place in the document it came from, so a caller never has
/// to guess which submessage was malformed.
///
/// `#[non_exhaustive]`: a codec added later adds a variant here — the `cbor` feature
/// already does — and that must not be a breaking change for a consumer that only ever
/// matched the JSON ones. Match with a `_` arm, or use [`ParseError::path`] and
/// `Display`, which every variant answers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The bytes are not JSON at all.
    #[error("body is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The bytes are not CBOR at all — ECMA-432 §7.1.
    ///
    /// ECMA-432 §11 says what an endpoint does about this: answer on the text fallback
    /// endpoint with an `Error` message recommending it. That is endpoint behaviour and
    /// lives with the binding; this variant is only the finding.
    #[cfg(feature = "cbor")]
    #[cfg_attr(docsrs, doc(cfg(feature = "cbor")))]
    #[error("body is not valid CBOR: {0}")]
    Cbor(#[from] ciborium::de::Error<std::io::Error>),

    /// A message or submessage is not an object (§5, §5.2).
    #[error("{path} must be a JSON object")]
    NotObject {
        /// Where in the message the offending element was.
        path: String,
    },

    /// A field the standard requires is absent (§5.1.2–5.1.4, §5.2.2–5.2.4).
    #[error("{path}: required field `{field}` is missing")]
    MissingField {
        /// Where in the message the offending element was.
        path: String,
        /// The canonical lower-case name of the missing field.
        field: &'static str,
    },

    /// A field is present but of the wrong type.
    #[error("{path}: field `{field}` must be {expected}, found {found}")]
    WrongType {
        /// Where in the message the offending element was.
        path: String,
        /// The canonical lower-case name of the field.
        field: Box<str>,
        /// What the standard asks for, e.g. `a string`.
        expected: &'static str,
        /// What arrived instead, e.g. `a number`.
        found: &'static str,
    },

    /// The same field arrived twice under different casings.
    ///
    /// §5 makes the two spellings the same field and says nothing about which value
    /// wins, so neither does this crate.
    #[error(
        "{path}: field `{field}` is present more than once under different casing ({keys:?}); \
         NLIP treats field names case-insensitively and the reference is ambiguous"
    )]
    DuplicateField {
        /// Where in the message the offending element was.
        path: String,
        /// The canonical lower-case name of the field.
        field: String,
        /// The spellings that arrived, sorted.
        keys: Vec<String>,
    },

    /// `format` is not one of the six values of Table 1 (§5.3).
    #[error("{path}: unsupported value {value:?} for `format`; expected {expected}")]
    UnknownFormat {
        /// Where in the message the offending element was.
        path: String,
        /// The value that arrived.
        value: String,
        /// The values Table 1 allows, as prose.
        expected: &'static str,
    },

    /// The content of a `binary` message on a JSON wire is not valid base64.
    ///
    /// §6.4 calls that spelling "JSON Base64 encoding" and ECMA-432 §7.2 requires it in
    /// the text fallback, so a payload that is not base64 cannot be interpreted at all
    /// — which is what a [`ParseError`] means. Over CBOR this cannot arise: §7.1 puts
    /// raw bytes in a byte string and a text string there is text.
    #[error("{path}: `binary` content on a JSON wire must be base64 (ECMA-430 §6.4): {source}")]
    Base64 {
        /// Where in the message the offending element was.
        path: String,
        /// What the base64 decoder objected to.
        #[source]
        source: base64::DecodeError,
    },

    /// A CBOR frame carries more than the one NLIP message it is allowed to.
    ///
    /// ECMA-432 §7.1: a binary message "shall contain a single NLIP message encoded in
    /// CBOR". A CBOR reader stops at the end of the first item, so two concatenated
    /// frames would otherwise decode as the first one and silently drop the second.
    /// The JSON wire needs no counterpart: `serde_json` already refuses trailing
    /// characters, and this is what makes the two wires answer alike.
    #[cfg(feature = "cbor")]
    #[cfg_attr(docsrs, doc(cfg(feature = "cbor")))]
    #[error(
        "a CBOR frame must carry a single NLIP message (ECMA-432 §7.1); \
         {remaining} byte(s) follow the first one"
    )]
    TrailingBytes {
        /// How many bytes were left after the first CBOR item.
        remaining: usize,
    },
}

/// What a document that could not be decoded at all was wrong about.
///
/// The vocabulary the conformance corpus fixes — `conformance/README.md`,
/// "Expectations" — and therefore the vocabulary a second implementation in another
/// language is measured against. It is *not* one case per [`ParseError`] variant, and
/// deliberately: the corpus names what an implementation must **distinguish**, so
/// ECMA-432 §7.1's "a frame carries one message and no more" is a
/// [`ParseErrorKind::Cbor`] failure here rather than a kind of its own.
///
/// Exhaustive, unlike [`ParseError`] itself, and that is what
/// [`ParseErrorKind::Unmapped`] is for: a variant added to the error type later falls
/// into it rather than making every consumer's `match` stop compiling. A consumer that
/// branches on all nine should keep being told when it has missed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// The body is not JSON at all. The path is always `$`.
    InvalidJson,
    /// The frame is not CBOR, carries a value NLIP has no room for — a tag, a map key
    /// that is not text, a non-finite float — or holds more than the single message
    /// ECMA-432 §7.1 allows. The path is always `$`.
    Cbor,
    /// Valid JSON or CBOR, but the message or a submessage is not an object.
    NotObject,
    /// A field §5.1.2–5.1.4 / §5.2.2–5.2.4 requires is absent, **or** is present as
    /// `null` where the clause requires a string.
    ///
    /// The second half is not a widening: `"subformat": null` names no subformat, and a
    /// document that says so is missing the field rather than carrying a wrong type for
    /// it. The interface contract's `parse-error-kind` (`wit/nlipsy.wit`,
    /// `missing-field`) states the same, and this doc follows it rather than the other
    /// way round, because the behaviour is the contract's.
    MissingField,
    /// A field is present with a type the standard does not allow there.
    WrongType,
    /// One field name arrived under two casings, which §5 makes one field with two
    /// values and nothing says which wins.
    DuplicateField,
    /// `format` is not one of Table 1's six values.
    UnknownFormat,
    /// `binary` content on a JSON wire is not decodable base64 (§6.4).
    Base64,
    /// A failure this vocabulary has no kind for.
    ///
    /// Never produced by [`ParseError::kind`], whose `match` is exhaustive and whose
    /// compiler error is what a new variant gets instead. It is here because the
    /// vocabulary is the *corpus's* and the interface contract's (`wit/nlipsy.wit`,
    /// `types.parse-error-kind`), and an implementation mapping some other error type
    /// onto it — a second implementation, a binding compiled against an older version of
    /// this crate — needs a total answer that is not a wrong one. A wrong kind is harder
    /// to notice than an unknown one.
    Unmapped,
}

impl ParseErrorKind {
    /// The corpus's spelling, as `expect.error.kind` writes it.
    ///
    /// Kebab-case and lower-case, which is the corpus's convention and not Rust's, so
    /// the one place the two spellings meet is here.
    pub const fn as_str(self) -> &'static str {
        match self {
            ParseErrorKind::InvalidJson => "invalid-json",
            ParseErrorKind::Cbor => "cbor",
            ParseErrorKind::NotObject => "not-object",
            ParseErrorKind::MissingField => "missing-field",
            ParseErrorKind::WrongType => "wrong-type",
            ParseErrorKind::DuplicateField => "duplicate-field",
            ParseErrorKind::UnknownFormat => "unknown-format",
            ParseErrorKind::Base64 => "base64",
            ParseErrorKind::Unmapped => "unmapped",
        }
    }
}

impl std::fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ParseError {
    /// Which kind of failure this is, in the conformance corpus's vocabulary.
    ///
    /// The mapping lives here rather than in the corpus runner because it is not the
    /// runner's opinion: it is what *this* implementation claims to have distinguished,
    /// and the JavaScript package's component (`wit/nlipsy.wit`,
    /// `types.parse-error-kind`) has to report the same nine values for the same bytes.
    /// Two copies of it would be two answers to one question.
    pub fn kind(&self) -> ParseErrorKind {
        match self {
            ParseError::Json(_) => ParseErrorKind::InvalidJson,
            #[cfg(feature = "cbor")]
            ParseError::Cbor(_) => ParseErrorKind::Cbor,
            // A frame holding more than one message is a `cbor` failure in this
            // vocabulary: nothing about the message is wrong, the frame is
            // (ECMA-432 §7.1).
            #[cfg(feature = "cbor")]
            ParseError::TrailingBytes { .. } => ParseErrorKind::Cbor,
            ParseError::NotObject { .. } => ParseErrorKind::NotObject,
            ParseError::MissingField { .. } => ParseErrorKind::MissingField,
            ParseError::WrongType { .. } => ParseErrorKind::WrongType,
            ParseError::DuplicateField { .. } => ParseErrorKind::DuplicateField,
            ParseError::UnknownFormat { .. } => ParseErrorKind::UnknownFormat,
            ParseError::Base64 { .. } => ParseErrorKind::Base64,
            // No `_` arm, and that is the point of putting the mapping here: `ParseError`
            // is `#[non_exhaustive]` to every other crate, so a mapping written outside
            // needs a catch-all and a variant added later lands in it silently. Inside,
            // a new variant stops this function compiling until somebody decides which
            // kind it is — and `ParseErrorKind::Unmapped` is what an implementation with
            // no such compiler check answers instead.
        }
    }

    /// Path of the offending element inside the message, e.g. `submessages[1]`.
    ///
    /// A failure of the document as a whole — invalid JSON or CBOR, or a frame holding
    /// more than one message — answers `$`: it has no position inside a message,
    /// because there is no message, or no single one.
    pub fn path(&self) -> &str {
        match self {
            ParseError::Json(_) => "$",
            #[cfg(feature = "cbor")]
            ParseError::Cbor(_) => "$",
            #[cfg(feature = "cbor")]
            ParseError::TrailingBytes { .. } => "$",
            ParseError::NotObject { path }
            | ParseError::MissingField { path, .. }
            | ParseError::WrongType { path, .. }
            | ParseError::DuplicateField { path, .. }
            | ParseError::UnknownFormat { path, .. }
            | ParseError::Base64 { path, .. } => path,
        }
    }
}

/// How badly a semantic rule of ECMA-430 was broken.
///
/// Exhaustive, like [`Format`](crate::Format): a third level would be a different
/// design, not a new case, and a consumer that branches on both should keep being told
/// when it has missed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The message can still be interpreted, but something is off-spec.
    Warning,
    /// The message cannot be interpreted as written.
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        })
    }
}

/// A semantic conformance finding: the message parsed, but broke a rule.
///
/// `rule` is a stable identifier (`"ecma430-5.3/binary-subformat"`) that tests and
/// dashboards can key on without parsing prose.
///
/// `#[non_exhaustive]`: a finding will want to carry more than it does today — the
/// clause it came from as data, a suggested repair — and adding that should not break
/// a consumer. Build one with [`Violation::error`] or [`Violation::warning`], and match
/// with `..`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Violation {
    /// How badly the rule was broken.
    pub severity: Severity,
    /// Where in the message the finding is, e.g. `submessages[1]`.
    pub path: String,
    /// Stable identifier of the rule, e.g. `ecma430-5.3/binary-subformat`.
    pub rule: &'static str,
    /// Human-readable explanation, naming the values involved.
    pub detail: String,
}

impl Violation {
    /// A finding that makes the message uninterpretable as written.
    pub fn error(path: impl Into<String>, rule: &'static str, detail: impl Into<String>) -> Self {
        Violation {
            severity: Severity::Error,
            path: path.into(),
            rule,
            detail: detail.into(),
        }
    }

    /// A finding the message survives: off-spec, but interpretable.
    pub fn warning(path: impl Into<String>, rule: &'static str, detail: impl Into<String>) -> Self {
        Violation {
            severity: Severity::Warning,
            path: path.into(),
            rule,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{}] at {}: {}",
            self.severity, self.rule, self.path, self.detail
        )
    }
}
