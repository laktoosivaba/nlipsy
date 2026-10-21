//! The contract's vocabulary and the engine's, in both directions.
//!
//! `wit/nlipsy.wit` is a *projection* of the Rust types this workspace already has —
//! `wit/README.md` §3 is the seed table, one row per item — so everything here is a
//! rename plus the three places the two shapes genuinely differ:
//!
//! 1. **`content.structured`.** WIT has no recursive types, so `Content::Array` and
//!    `Content::Object` cross as the canonical JSON text of the value
//!    (`Content::to_json_value`, `crates/nlipsy-protocol/src/content.rs:201`) and are
//!    read back with `serde_json`. The producer of that string is always the glue and
//!    never a user, so a string that is not JSON is a host
//!    bug — and the contract's defined answer is to take it as `text`, which invents
//!    nothing, discards nothing, and comes back out of `encode` unchanged.
//! 2. **Durations.** `Duration` on one side, milliseconds as `u64` on the other, both
//!    ways, everywhere the two meet.
//! 3. **`&'static str` rule ids.** `Violation::rule` is a `&'static str` because every
//!    id this workspace produces is a constant of `Message::validate`. A host's is a
//!    `String` that arrived at run time, so the way *in* cannot be exact — see
//!    [`violation_in`], which is called on one path and never on the one that would
//!    corrupt a round trip.
//!
//! Naming: `_out` is engine → contract, `_in` is contract → engine, for every item.
//!
//! Several of the engine's enums are `#[non_exhaustive]` while the contract's are
//! closed — `wit/README.md` §7 states that asymmetry — so each `_out` here has a `_`
//! arm, and each says what it answers and why that answer is the safe direction.

use std::time::Duration;

use nlipsy_core::{
    Body, ConformityConfig, Disposition, EndpointError, Exchange, ExchangeDetail, Finding,
    Findings, Finished, Outcome, Owed, Policy, Rejected, Rejection, Reply, Served, TokenKind,
    TransportError,
};
use nlipsy_http::http::StatusCode;
use nlipsy_http::{CORRELATOR_HEADER, DEFAULT_MAX_REQUEST_BODY_SIZE, StatusMap};
use nlipsy_protocol::error::ParseErrorKind;
use nlipsy_protocol::{
    Content, Format, Message, ParseError, Severity, Submessage, Violation, Wire,
};

use crate::bindings::nlipsy::engine::types as wt;

/// The rule id a host's own clause-5 finding is carried under.
///
/// [`Violation::rule`] is a `&'static str`, and a host's is not: interning one would
/// leak a string per distinct value on a per-request path, and picking a neighbouring
/// id would be wrong data, which is worse. So the host's id travels in the prose —
/// where the one reader of a reconstructed violation, `Outcome::public_message`, prints
/// it — and the id field says truthfully where the finding came from rather than
/// claiming to be a clause-5 rule this engine applied.
///
/// It is reachable from exactly one place: `endpoint-error.refused(violations(..))`,
/// a refusal a host made on its own. The findings a host hands back on the *success*
/// path — `finished.findings` — are never converted at all, for precisely this reason;
/// see `src/server.rs`.
const HOST_VIOLATION: &str = "nlipsy-component/host-violation";

// ---------------------------------------------------------------------------
// Durations
// ---------------------------------------------------------------------------

/// A delay as the contract carries it: whole milliseconds.
///
/// Saturating rather than wrapping: a delay longer than 584 million years is not a
/// number a peer can act on, and the alternative — `as u64` — would turn it into a
/// small one.
pub fn millis_out(delay: Duration) -> u64 {
    u64::try_from(delay.as_millis()).unwrap_or(u64::MAX)
}

/// A delay as the engine carries it.
pub fn millis_in(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

// ---------------------------------------------------------------------------
// The message model — ECMA-430 clause 5
// ---------------------------------------------------------------------------

/// Which encoding — `crates/nlipsy-protocol/src/content.rs:78`.
pub fn wire_in(wire: wt::Wire) -> Wire {
    match wire {
        wt::Wire::Json => Wire::Json,
        wt::Wire::Cbor => Wire::Cbor,
    }
}

/// ECMA-430 Table 1, contract to engine. Total in both directions, which is the whole
/// reason `format` is an enum and not a string: a string would make this conversion
/// partial, with nowhere to report a value the model cannot hold.
pub fn format_in(format: wt::Format) -> Format {
    match format {
        wt::Format::Text => Format::Text,
        wt::Format::Token => Format::Token,
        wt::Format::Structured => Format::Structured,
        wt::Format::Binary => Format::Binary,
        wt::Format::Location => Format::Location,
        wt::Format::Generic => Format::Generic,
    }
}

/// ECMA-430 Table 1, engine to contract.
pub fn format_out(format: Format) -> wt::Format {
    match format {
        Format::Text => wt::Format::Text,
        Format::Token => wt::Format::Token,
        Format::Structured => wt::Format::Structured,
        Format::Binary => wt::Format::Binary,
        Format::Location => wt::Format::Location,
        Format::Generic => wt::Format::Generic,
    }
}

/// §5.1.4's value, contract to engine.
///
/// `structured` is read with `serde_json` and, when the text is not JSON, taken as
/// `text` — the fallback the contract defines on `types.content`. Nothing is invented
/// and nothing is discarded: the same characters come back out of `encode`.
pub fn content_in(content: wt::Content) -> Content {
    match content {
        wt::Content::Null => Content::Null,
        wt::Content::Bool(value) => Content::Bool(value),
        wt::Content::Number(number) => Content::Number(number_in(number)),
        wt::Content::Text(text) => Content::String(text),
        wt::Content::Bytes(bytes) => Content::Bytes(bytes),
        wt::Content::Structured(json) => match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(value) => Content::from(value),
            Err(_) => Content::String(json),
        },
    }
}

/// §5.1.4's value, engine to contract.
pub fn content_out(content: &Content) -> wt::Content {
    match content {
        Content::Null => wt::Content::Null,
        Content::Bool(value) => wt::Content::Bool(*value),
        Content::Number(number) => wt::Content::Number(number_out(number)),
        Content::String(text) => wt::Content::Text(text.clone()),
        Content::Bytes(bytes) => wt::Content::Bytes(bytes.clone()),
        // The one recursive pair, flattened. `to_json_value` sorts object keys (a
        // `BTreeMap` and an unordered ECMA-404 object) and spells a nested byte string
        // as base64, a loss JSON has anyway: no corpus case nests a byte string inside
        // structured content, and one that did would need a corpus convention first.
        Content::Array(_) | Content::Object(_) => {
            wt::Content::Structured(content.to_json_value().to_string())
        }
    }
}

/// ECMA-404's number space, contract to engine.
///
/// A `float` that is not finite cannot arrive — ECMA-432 §7.1 refuses NaN and the
/// infinities at the decoder and `serde_json::Number` has no room for them — so the
/// answer for one is `null`: the only total answer, and the same one the CBOR decoder
/// would have given the peer.
fn number_in(number: wt::Number) -> serde_json::Number {
    match number {
        wt::Number::Int(value) => value.into(),
        wt::Number::Uint(value) => value.into(),
        wt::Number::Float(value) => serde_json::Number::from_f64(value).unwrap_or_else(|| 0.into()),
    }
}

/// ECMA-404's number space, engine to contract.
fn number_out(number: &serde_json::Number) -> wt::Number {
    if let Some(value) = number.as_i64() {
        wt::Number::Int(value)
    } else if let Some(value) = number.as_u64() {
        wt::Number::Uint(value)
    } else {
        // `Number` holds an `i64`, a `u64` or a finite `f64` and nothing else, so this
        // arm is the third of three rather than a fallback.
        wt::Number::Float(number.as_f64().unwrap_or_default())
    }
}

/// §5.2, contract to engine.
pub fn submessage_in(submessage: wt::Submessage) -> Submessage {
    let mut out = Submessage::new(
        format_in(submessage.format),
        submessage.subformat,
        content_in(submessage.content),
    );
    out.label = submessage.label;
    out
}

/// §5.2, engine to contract.
pub fn submessage_out(submessage: &Submessage) -> wt::Submessage {
    wt::Submessage {
        label: submessage.label.clone(),
        format: format_out(submessage.format),
        subformat: submessage.subformat.clone(),
        content: content_out(&submessage.content),
    }
}

/// §5.1, contract to engine.
pub fn message_in(message: wt::Message) -> Message {
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

/// §5.1, engine to contract.
pub fn message_out(message: &Message) -> wt::Message {
    wt::Message {
        message_type: message.message_type.clone(),
        control: message.control,
        label: message.label.clone(),
        format: format_out(message.format),
        subformat: message.subformat.clone(),
        content: content_out(&message.content),
        submessages: message.submessages.iter().map(submessage_out).collect(),
    }
}

// ---------------------------------------------------------------------------
// What was wrong with a document, and with a message
// ---------------------------------------------------------------------------

/// The corpus's nine kinds — `ParseError::kind`, lifted into `nlipsy-protocol` so that
/// the runner and this guest answer one question once.
pub fn parse_error_kind_out(kind: ParseErrorKind) -> wt::ParseErrorKind {
    match kind {
        ParseErrorKind::InvalidJson => wt::ParseErrorKind::InvalidJson,
        ParseErrorKind::Cbor => wt::ParseErrorKind::Cbor,
        ParseErrorKind::NotObject => wt::ParseErrorKind::NotObject,
        ParseErrorKind::MissingField => wt::ParseErrorKind::MissingField,
        ParseErrorKind::WrongType => wt::ParseErrorKind::WrongType,
        ParseErrorKind::DuplicateField => wt::ParseErrorKind::DuplicateField,
        ParseErrorKind::UnknownFormat => wt::ParseErrorKind::UnknownFormat,
        ParseErrorKind::Base64 => wt::ParseErrorKind::Base64,
        ParseErrorKind::Unmapped => wt::ParseErrorKind::Unmapped,
    }
}

/// A decode failure, in the three fields the corpus compares plus the prose.
///
/// The *field* is derived here and not in `nlipsy-protocol`, which is where the *kind*
/// now lives: the mapping is `describe`'s (`crates/conformance/src/lib.rs`), and the
/// two are kept in step by the corpus itself — the `component` mode of
/// `nlipsy-conformance` asserts this record against the twenty-nine cases that carry
/// an `expect.error` block, so a divergence fails a test rather than shipping.
///
/// Two variants carry no field of their own and one is supplied, because it is the only
/// field the variant can be about: `UnknownFormat` is always about `format`, and
/// `Base64` always about `content` (§6.4 spells only a `binary` content in base64).
pub fn parse_error_out(error: &ParseError) -> wt::ParseError {
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
    wt::ParseError {
        kind: parse_error_kind_out(error.kind()),
        path: error.path().to_owned(),
        field,
        detail: error.to_string(),
    }
}

/// §5's two severities, engine to contract.
pub fn severity_out(severity: Severity) -> wt::Severity {
    match severity {
        Severity::Error => wt::Severity::Error,
        Severity::Warning => wt::Severity::Warning,
    }
}

/// One clause-5 finding, engine to contract.
pub fn violation_out(violation: &Violation) -> wt::Violation {
    wt::Violation {
        severity: severity_out(violation.severity),
        path: violation.path.clone(),
        rule: violation.rule.to_owned(),
        detail: violation.detail.clone(),
    }
}

/// One clause-5 finding a *host* reported, contract to engine.
///
/// The one lossy conversion in this file, and the loss is the rule id: see
/// [`HOST_VIOLATION`] for why it cannot be carried in the field the engine has for it,
/// and for the single path that reaches this function.
pub fn violation_in(violation: wt::Violation) -> Violation {
    let detail = format!("[{}] {}", violation.rule, violation.detail);
    match violation.severity {
        wt::Severity::Error => Violation::error(violation.path, HOST_VIOLATION, detail),
        wt::Severity::Warning => Violation::warning(violation.path, HOST_VIOLATION, detail),
    }
}

/// What a finding does to an exchange, engine to contract.
pub fn disposition_out(disposition: Disposition) -> wt::Disposition {
    match disposition {
        Disposition::Reject => wt::Disposition::Reject,
        Disposition::Log => wt::Disposition::Log,
        Disposition::Ignore => wt::Disposition::Ignore,
    }
}

/// What a finding does to an exchange, contract to engine.
pub fn disposition_in(disposition: wt::Disposition) -> Disposition {
    match disposition {
        wt::Disposition::Reject => Disposition::Reject,
        wt::Disposition::Log => Disposition::Log,
        wt::Disposition::Ignore => Disposition::Ignore,
    }
}

/// One finding and what the policy decided, engine to contract.
pub fn finding_out(finding: &Finding) -> wt::Finding {
    wt::Finding {
        violation: violation_out(&finding.violation),
        disposition: disposition_out(finding.disposition),
    }
}

/// A step's findings, engine to contract, in the order the step reported them.
pub fn findings_out(findings: &Findings) -> Vec<wt::Finding> {
    findings.iter().map(finding_out).collect()
}

/// A request clause 6 will not answer, engine to contract.
pub fn rejected_out(rejected: &Rejected) -> wt::Rejected {
    match rejected {
        Rejected::Violations(violations) => {
            wt::Rejected::Violations(violations.iter().map(violation_out).collect())
        }
        Rejected::TooManyTokens { count, limit } => wt::Rejected::TooManyTokens(wt::TokenCeiling {
            count: saturating(*count),
            limit: saturating(*limit),
        }),
        // `Rejected` is `#[non_exhaustive]` and this contract's version has two cases.
        // A guard added since is reported as what it is rather than mapped onto the
        // ceiling, which a peer would act on by sending fewer tokens.
        _ => wt::Rejected::Violations(vec![wt::Violation {
            severity: wt::Severity::Error,
            path: "$".to_owned(),
            rule: "nlipsy-component/unmapped-refusal".to_owned(),
            detail: rejected.to_string(),
        }]),
    }
}

/// A request clause 6 will not answer, contract to engine.
pub fn rejected_in(rejected: wt::Rejected) -> Rejected {
    match rejected {
        wt::Rejected::Violations(violations) => {
            Rejected::Violations(violations.into_iter().map(violation_in).collect())
        }
        wt::Rejected::TooManyTokens(ceiling) => Rejected::TooManyTokens {
            count: ceiling.count as usize,
            limit: ceiling.limit as usize,
        },
    }
}

/// A count the contract carries as a `u32`.
///
/// Saturating: the counts are token submessages in one message, so the ceiling is
/// unreachable in practice, and a wrapping cast would turn "far too many" into "few".
pub fn saturating(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A reply that has been through `finish`, engine to contract.
pub fn finished_out(finished: &Finished) -> wt::Finished {
    wt::Finished {
        message: message_out(&finished.message),
        findings: findings_out(&finished.findings),
        withheld: finished.withheld(),
    }
}

// ---------------------------------------------------------------------------
// Clause 6's configuration
// ---------------------------------------------------------------------------

/// §6.2's two reserved kinds, engine to contract.
pub fn token_kind_out(kind: TokenKind) -> wt::TokenKind {
    match kind {
        TokenKind::Conversation => wt::TokenKind::Conversation,
        TokenKind::Authentication => wt::TokenKind::Authentication,
    }
}

/// What an end-point does about the optional parts of clause 6, contract to engine.
pub fn config_in(config: wt::Config) -> ConformityConfig {
    ConformityConfig::new()
        .with_conversation_token(config.issue_conversation_token)
        .with_authentication_token(config.issue_authentication_token)
        .with_max_tokens_per_message(config.max_tokens_per_message as usize)
}

/// What an end-point does about the optional parts of clause 6, engine to contract.
pub fn config_out(config: &ConformityConfig) -> wt::Config {
    wt::Config {
        issue_conversation_token: config.issue_conversation_token,
        issue_authentication_token: config.issue_authentication_token,
        max_tokens_per_message: saturating(config.max_tokens_per_message),
    }
}

/// One direction's clause-5 policy, contract to engine.
pub fn policy_in(policy: wt::Policy) -> Policy {
    Policy::new(
        disposition_in(policy.on_error),
        disposition_in(policy.on_warning),
    )
}

/// One direction's clause-5 policy, engine to contract.
pub fn policy_out(policy: &Policy) -> wt::Policy {
    wt::Policy {
        on_error: disposition_out(policy.on_error),
        on_warning: disposition_out(policy.on_warning),
    }
}

/// Both directions' policies, contract to engine.
pub fn rejection_in(rejection: wt::Rejection) -> Rejection {
    Rejection::new(policy_in(rejection.inbound), policy_in(rejection.outbound))
}

/// Both directions' policies, engine to contract.
pub fn rejection_out(rejection: &Rejection) -> wt::Rejection {
    wt::Rejection {
        inbound: policy_out(&rejection.inbound),
        outbound: policy_out(&rejection.outbound),
    }
}

// ---------------------------------------------------------------------------
// How an exchange ended
// ---------------------------------------------------------------------------

/// Why an end-point could not answer, contract to engine.
pub fn endpoint_error_in(error: wt::EndpointError) -> EndpointError {
    match error {
        wt::EndpointError::Unsupported(detail) => EndpointError::Unsupported(detail),
        wt::EndpointError::Unavailable(unavailable) => EndpointError::Unavailable {
            detail: unavailable.detail,
            retry_after: unavailable.retry_after_ms.map(millis_in),
        },
        wt::EndpointError::Internal(detail) => EndpointError::Internal(detail),
        wt::EndpointError::Refused(rejected) => EndpointError::Refused(rejected_in(rejected)),
        wt::EndpointError::TimedOut(ms) => EndpointError::TimedOut {
            after: millis_in(ms),
        },
    }
}

/// Why an end-point could not answer, engine to contract.
pub fn endpoint_error_out(error: &EndpointError) -> wt::EndpointError {
    match error {
        EndpointError::Unsupported(detail) => wt::EndpointError::Unsupported(detail.clone()),
        EndpointError::Unavailable {
            detail,
            retry_after,
        } => wt::EndpointError::Unavailable(wt::Unavailable {
            retry_after_ms: retry_after.map(millis_out),
            detail: detail.clone(),
        }),
        EndpointError::TimedOut { after } => wt::EndpointError::TimedOut(millis_out(*after)),
        EndpointError::Internal(detail) => wt::EndpointError::Internal(detail.clone()),
        EndpointError::Refused(rejected) => wt::EndpointError::Refused(rejected_out(rejected)),
        // `EndpointError` is `#[non_exhaustive]`. `internal` is the one case whose
        // detail the contract already says is for the log and never for the peer
        // (`engine.public-message` strips it), so a failure this version has no name for
        // is reported as this end-point's fault, which is what it is.
        _ => wt::EndpointError::Internal(error.to_string()),
    }
}

/// How one exchange ended, engine to contract.
pub fn outcome_out(outcome: &Outcome) -> wt::Outcome {
    match outcome {
        Outcome::Ok => wt::Outcome::Ok,
        Outcome::Malformed(error) => wt::Outcome::Malformed(parse_error_out(error)),
        Outcome::Refused(rejected) => wt::Outcome::Refused(rejected_out(rejected)),
        Outcome::Failed(error) => wt::Outcome::Failed(endpoint_error_out(error)),
        // `Outcome` is `#[non_exhaustive]`, and every arm of it produces a reply, so
        // there is always something to hand back: a way of ending this version of the
        // contract has no case for is the end-point's own fault and not the peer's,
        // which is exactly what `failed(internal(..))` says.
        _ => wt::Outcome::Failed(wt::EndpointError::Internal(
            "the engine reported an outcome this version of nlipsy:engine has no case for"
                .to_owned(),
        )),
    }
}

/// What clause 6 still owes, engine to contract.
pub fn owed_out(owed: Owed) -> wt::Owed {
    match owed {
        Owed::Nothing => wt::Owed::Nothing,
        Owed::Clause6 => wt::Owed::Clause6,
        Owed::ControlOnly => wt::Owed::ControlOnly,
        // `Owed` is `#[non_exhaustive]`. Answering `clause6` for an obligation this
        // version cannot name would discharge the *wrong* one; `nothing` discharges
        // none, which is the only answer that adds nothing a peer could mistake for an
        // echo.
        _ => wt::Owed::Nothing,
    }
}

/// What clause 6 still owes, contract to engine.
pub fn owed_in(owed: wt::Owed) -> Owed {
    match owed {
        wt::Owed::Nothing => Owed::Nothing,
        wt::Owed::Clause6 => Owed::Clause6,
        wt::Owed::ControlOnly => Owed::ControlOnly,
    }
}

/// One exchange, answered, engine to contract.
pub fn served_out(served: &Served) -> wt::Served {
    wt::Served {
        reply: message_out(&served.reply),
        outcome: outcome_out(&served.outcome),
        withheld: served.withheld,
        findings: findings_out(&served.findings),
    }
}

/// A contract `outcome` as the engine's, for the rules that are functions of one.
///
/// `engine.owed-by` and `engine.public-message` are exported as *rules*, and
/// `StatusMap::of` is one the handler applies, so each is answered by calling the engine
/// rather than by restating its `match` (`crates/nlipsy-core/src/serve.rs:168`, `:84`
/// and `crates/nlipsy-http/src/status.rs:138`). Rebuilding the value they take is exact for
/// three of its four cases and cannot be for `malformed`: no [`ParseError`] variant has
/// a free-form `Display`, so the prose the contract carries in `parse-error.detail`
/// cannot be put back into one.
///
/// So this is sound for a rule that reads the *variant* — `owed` and `status-of` both
/// do, and `status-of` reads the inner variant of `failed`, which is exact — and is not
/// for one that reads the `malformed` payload. `public-message` is the one that does,
/// and its `malformed` arm is answered from the record in `src/server.rs` rather than
/// through here. Nothing built here escapes the call it was built for.
pub fn outcome_for_rules(outcome: wt::Outcome) -> Outcome {
    match outcome {
        wt::Outcome::Ok => Outcome::Ok,
        wt::Outcome::Malformed(error) => {
            Outcome::Malformed(ParseError::NotObject { path: error.path })
        }
        wt::Outcome::Refused(rejected) => Outcome::Refused(rejected_in(rejected)),
        wt::Outcome::Failed(error) => Outcome::Failed(endpoint_error_in(error)),
    }
}

// ---------------------------------------------------------------------------
// The client half
// ---------------------------------------------------------------------------

/// What a binding knows about one exchange, contract to engine.
pub fn exchange_detail_in(detail: wt::ExchangeDetail) -> ExchangeDetail {
    match detail {
        wt::ExchangeDetail::Http(http) => ExchangeDetail::Http {
            status: http.status,
            retry_after: http.retry_after_ms.map(millis_in),
        },
    }
}

/// What a binding knows about one exchange, engine to contract.
pub fn exchange_detail_out(detail: &ExchangeDetail) -> wt::ExchangeDetail {
    match detail {
        ExchangeDetail::Http {
            status,
            retry_after,
        } => wt::ExchangeDetail::Http(wt::HttpDetail {
            status: *status,
            retry_after_ms: retry_after.map(millis_out),
        }),
        // `ExchangeDetail` is `#[non_exhaustive]` and ECMA-432 and ECMA-433 each add a
        // case when their bindings arrive (`wit/README.md` §7). Until then a binding
        // this contract cannot name has no status, and `0` is not one: `is-ok` reads
        // 200..300, so a caller treats it as "did not answer", which is the safe
        // direction for a reply nobody can classify.
        _ => wt::ExchangeDetail::Http(wt::HttpDetail {
            status: 0,
            retry_after_ms: None,
        }),
    }
}

/// What a transport carried back, contract to engine.
pub fn exchange_in(exchange: wt::Exchange) -> Exchange {
    Exchange::new(exchange.body, exchange_detail_in(exchange.detail))
}

/// Why an exchange produced no reply, contract to engine.
pub fn transport_error_in(error: wt::TransportError) -> TransportError {
    match error {
        wt::TransportError::Unreachable(failure) => TransportError::Unreachable {
            endpoint: failure.endpoint,
            detail: failure.detail,
        },
        wt::TransportError::Interrupted(failure) => TransportError::Interrupted {
            endpoint: failure.endpoint,
            detail: failure.detail,
        },
        wt::TransportError::Malformed(detail) => TransportError::Malformed { detail },
    }
}

/// Why an exchange produced no reply, engine to contract.
pub fn transport_error_out(error: &TransportError) -> wt::TransportError {
    match error {
        TransportError::Unreachable { endpoint, detail } => {
            wt::TransportError::Unreachable(wt::TransportFailure {
                endpoint: endpoint.clone(),
                detail: detail.clone(),
            })
        }
        TransportError::Interrupted { endpoint, detail } => {
            wt::TransportError::Interrupted(wt::TransportFailure {
                endpoint: endpoint.clone(),
                detail: detail.clone(),
            })
        }
        TransportError::Malformed { detail } => wt::TransportError::Malformed(detail.clone()),
        // `TransportError` is `#[non_exhaustive]`. `malformed` is the case that names no
        // end-point and promises nothing about whether the request was seen, which is
        // the only honest thing to say about a failure this version cannot name — the
        // other two would tell a caller it is safe to retry, or that it is not.
        _ => wt::TransportError::Malformed(error.to_string()),
    }
}

/// What the end-point sent back, engine to contract.
pub fn body_out(body: &Body) -> wt::Body {
    match body {
        Body::Message(message) => wt::Body::Message(message_out(message)),
        Body::Opaque { text, why } => wt::Body::Opaque(wt::Opaque {
            text: text.clone(),
            why: why.clone(),
        }),
    }
}

/// One reply, engine to contract, with the four derived views computed in Rust.
///
/// `text`, `tokens`, `conversation-tokens`, `authentication-tokens` and `is-error` are
/// `Reply`'s own methods (`crates/nlipsy-core/src/transport.rs:328`, `:363`, `:386`,
/// `:399`, `:417`) rather than fields the wrapper would re-derive: that is the point of
/// their being in the record. `tokens` is the one that could not be derived honestly at
/// all — §6.2 ¶2's list chains `first_submessage_token` in front of `Message::tokens`,
/// and a wrapper filtering `submessages` on `format == token` would drop the token of a
/// message that *is* one.
pub fn reply_out(reply: &Reply) -> wt::Reply {
    wt::Reply {
        detail: exchange_detail_out(&reply.detail),
        bytes: saturating(reply.bytes),
        body: body_out(&reply.body),
        text: reply.text(),
        tokens: reply.tokens().iter().map(submessage_out).collect(),
        conversation_tokens: reply.conversation_tokens(),
        authentication_tokens: reply.authentication_tokens(),
        is_error: reply.is_error(),
    }
}

// ---------------------------------------------------------------------------
// ECMA-431 §6 as the host configures it
// ---------------------------------------------------------------------------

/// The deployment's codes, engine to contract.
pub fn status_map_out(statuses: &StatusMap) -> wt::StatusMap {
    wt::StatusMap {
        malformed: statuses.malformed.as_u16(),
        refused: statuses.refused.as_u16(),
        too_large: statuses.too_large.as_u16(),
        unsupported: statuses.unsupported.as_u16(),
        unavailable: statuses.unavailable.as_u16(),
        timed_out: statuses.timed_out.as_u16(),
        internal: statuses.internal.as_u16(),
        withheld: statuses.withheld.as_u16(),
        other: statuses.other.as_u16(),
    }
}

/// The deployment's codes, contract to engine.
///
/// A number outside RFC 9110's 100..600 is not a status, and the host had nowhere to
/// learn that: `host-endpoint.http-binding` returns a record and the contract gives it
/// no error arm, as it gives none to any value the host itself produced: a field the
/// host misspelled is a host bug and not peer input. So a field the host spelled
/// wrongly falls back to *that field's* default, which is the reference server's code
/// for it (`crates/nlipsy-http/src/status.rs:105`) and is what a deployment that had
/// said nothing would have got. Trapping instead would kill an instance that may be serving
/// other exchanges, over a configuration value.
pub fn status_map_in(statuses: wt::StatusMap) -> StatusMap {
    let default = StatusMap::default();
    StatusMap {
        malformed: status(statuses.malformed, default.malformed),
        refused: status(statuses.refused, default.refused),
        too_large: status(statuses.too_large, default.too_large),
        unsupported: status(statuses.unsupported, default.unsupported),
        unavailable: status(statuses.unavailable, default.unavailable),
        timed_out: status(statuses.timed_out, default.timed_out),
        internal: status(statuses.internal, default.internal),
        withheld: status(statuses.withheld, default.withheld),
        other: status(statuses.other, default.other),
    }
}

fn status(code: u16, fallback: StatusCode) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(fallback)
}

/// The reference server's binding, option for option — what
/// `engine.default-http-options` answers and what a host that has changed nothing
/// returns from `host-endpoint.http-binding`.
///
/// Assembled from `nlipsy-http`'s own values rather than from `ServerConfig::default`
/// (`crates/nlipsy-server/src/server.rs:103`), because that crate is axum over hyper
/// over tokio and does not compile for `wasm32-wasip3`. The two cannot disagree:
/// `ServerConfig::default` is `DEFAULT_MAX_REQUEST_BODY_SIZE`, `CORRELATOR_HEADER`,
/// `ConformityConfig::default`, `Rejection::default` and `StatusMap::default`, and the
/// first, the second and the last are exactly these three. `cors` is `None`, which is
/// also what that binding does — it serves no browser and has none.
pub fn http_options_out() -> wt::HttpOptions {
    wt::HttpOptions {
        // 8 MiB, which fits a `u32` with room to spare; the clamp is the cast being
        // written down rather than a case that can happen.
        max_request_body_size: saturating(DEFAULT_MAX_REQUEST_BODY_SIZE),
        correlator_header: Some(CORRELATOR_HEADER.to_owned()),
        cors: None,
        status: status_map_out(&StatusMap::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use nlipsy_protocol::Submessage;

    /// A message with one of everything the boundary has to carry.
    fn rich() -> Message {
        let mut message = Message::new(Format::Text, "english", "hello");
        message.message_type = Some("Error".to_owned());
        message.control = Some(true);
        message.label = Some("turn-1".to_owned());
        message.submessages = vec![
            Submessage::token("conversation", "c-1").with_label("from-the-peer"),
            Submessage::new(
                Format::Binary,
                "image/png",
                Content::Bytes(vec![0, 1, 2, 255]),
            ),
            Submessage::new(
                Format::Structured,
                "json",
                Content::Object(BTreeMap::from([
                    ("b".to_owned(), Content::Number(2.into())),
                    ("a".to_owned(), Content::Array(vec![Content::Null])),
                ])),
            ),
        ];
        message
    }

    #[test]
    fn a_message_survives_the_round_trip_whole() {
        let message = rich();
        assert_eq!(message_in(message_out(&message)), message);
    }

    #[test]
    fn table_one_is_total_in_both_directions() {
        for format in Format::ALL {
            assert_eq!(format_in(format_out(format)), format);
        }
    }

    #[test]
    fn structured_content_crosses_as_canonical_json_and_comes_back() {
        let content = Content::Object(BTreeMap::from([
            ("z".to_owned(), Content::Bool(false)),
            (
                "a".to_owned(),
                Content::Array(vec![Content::Number(1.into())]),
            ),
        ]));

        let wt::Content::Structured(json) = content_out(&content) else {
            panic!("an object crosses as `structured`");
        };
        // Sorted keys, compact, no spaces: `Content::Object` is a `BTreeMap` and
        // ECMA-404 makes an object unordered, so the canonical text is the sorted one.
        assert_eq!(json, r#"{"a":[1],"z":false}"#);
        assert_eq!(content_in(wt::Content::Structured(json)), content);
    }

    #[test]
    fn a_structured_string_that_is_not_json_is_read_as_text() {
        // The invariant on `types.content` is the host's: the producer is always the
        // glue. The contract's answer for a host that breaks it is this — not a trap,
        // not an invented value, and nothing discarded, since the same characters come
        // back out of `encode`.
        let broken = "this was never JSON".to_owned();
        assert_eq!(
            content_in(wt::Content::Structured(broken.clone())),
            Content::String(broken)
        );
    }

    #[test]
    fn a_number_keeps_the_precision_the_three_cases_exist_for() {
        for content in [
            Content::Number(i64::MIN.into()),
            Content::Number(u64::MAX.into()),
            Content::Number(serde_json::Number::from_f64(1.5).expect("finite")),
        ] {
            assert_eq!(content_in(content_out(&content)), content);
        }

        // The three-way split is what stops an `i64` and a `u64` going through an `f64`:
        // both of these are exactly representable here and neither is as a double.
        assert!(matches!(
            content_out(&Content::Number(i64::MIN.into())),
            wt::Content::Number(wt::Number::Int(i64::MIN))
        ));
        assert!(matches!(
            content_out(&Content::Number(u64::MAX.into())),
            wt::Content::Number(wt::Number::Uint(u64::MAX))
        ));
    }

    #[test]
    fn bytes_cross_as_bytes_and_are_never_base64_on_the_way() {
        // The distinction the corpus spells `{"$bytes": …}`: `content.bytes` and
        // `content.text` are different cases at this boundary, which is the whole point
        // of the marker.
        let bytes = Content::Bytes(vec![0xde, 0xad, 0xbe, 0xef]);
        assert!(matches!(content_out(&bytes), wt::Content::Bytes(_)));
        assert_eq!(content_in(content_out(&bytes)), bytes);
    }

    #[test]
    fn a_decode_failure_names_the_kind_the_path_and_the_field() {
        // The three fields `conformance/cases/` asserts, on one document per kind that
        // the JSON wire can produce. The corpus itself is what checks the whole mapping,
        // through the `component` mode of `nlipsy-conformance`; this is the boundary's
        // own share of it.
        let not_json = Message::from_json_bytes(b"{").expect_err("not a document");
        let described = parse_error_out(&not_json);
        assert_eq!(described.kind, wt::ParseErrorKind::InvalidJson);
        assert_eq!(described.path, "$");
        assert_eq!(described.field, None);
        assert!(!described.detail.is_empty(), "the prose is the `Display`");

        let missing = Message::from_json_bytes(br#"{"subformat":"english"}"#)
            .expect_err("§5.1.2 requires `format`");
        let described = parse_error_out(&missing);
        assert_eq!(described.kind, wt::ParseErrorKind::MissingField);
        assert_eq!(described.field.as_deref(), Some("format"));

        let unknown =
            Message::from_json_bytes(br#"{"format":"sideways","subformat":"x","content":"y"}"#)
                .expect_err("not one of Table 1's six");
        let described = parse_error_out(&unknown);
        assert_eq!(described.kind, wt::ParseErrorKind::UnknownFormat);
        // Supplied rather than carried: the variant can only be about `format`.
        assert_eq!(described.field.as_deref(), Some("format"));

        let not_base64 = Message::from_json_bytes(
            br#"{"format":"binary","subformat":"image/png","content":"!!!!"}"#,
        )
        .expect_err("§6.4 spells binary content in base64");
        let described = parse_error_out(&not_base64);
        assert_eq!(described.kind, wt::ParseErrorKind::Base64);
        assert_eq!(described.field.as_deref(), Some("content"));

        // ECMA-432 §7.1's "one message per frame" is a `cbor` failure in this
        // vocabulary and not a kind of its own — the corpus names what an
        // implementation must *distinguish*.
        let two_frames = [b"\xa3".to_vec(), vec![0xa0]].concat();
        let trailing = Message::from_cbor_slice(&two_frames).expect_err("not one message");
        assert_eq!(parse_error_out(&trailing).kind, wt::ParseErrorKind::Cbor);
    }

    #[test]
    fn every_kind_of_the_vocabulary_has_a_case_on_the_boundary() {
        // `ParseErrorKind` is exhaustive and the contract's enum is closed at the same
        // nine, which is what makes this a total function rather than one with a
        // fallback. Listing them is how a ninth case added to either side fails here.
        use ParseErrorKind as Kind;
        for (kind, spelt) in [
            (Kind::InvalidJson, wt::ParseErrorKind::InvalidJson),
            (Kind::Cbor, wt::ParseErrorKind::Cbor),
            (Kind::NotObject, wt::ParseErrorKind::NotObject),
            (Kind::MissingField, wt::ParseErrorKind::MissingField),
            (Kind::WrongType, wt::ParseErrorKind::WrongType),
            (Kind::DuplicateField, wt::ParseErrorKind::DuplicateField),
            (Kind::UnknownFormat, wt::ParseErrorKind::UnknownFormat),
            (Kind::Base64, wt::ParseErrorKind::Base64),
            (Kind::Unmapped, wt::ParseErrorKind::Unmapped),
        ] {
            assert_eq!(parse_error_kind_out(kind), spelt, "{}", kind.as_str());
        }
    }

    #[test]
    fn a_delay_crosses_as_whole_milliseconds_both_ways() {
        assert_eq!(millis_out(Duration::from_millis(1500)), 1500);
        assert_eq!(millis_in(1500), Duration::from_millis(1500));
        // Saturating rather than wrapping: a delay no peer can act on must not come out
        // the other side as a short one.
        assert_eq!(millis_out(Duration::MAX), u64::MAX);
    }

    #[test]
    fn an_endpoint_failure_survives_the_round_trip() {
        for error in [
            EndpointError::Unsupported("no video".to_owned()),
            EndpointError::Unavailable {
                detail: "the model provider answered 429".to_owned(),
                retry_after: Some(Duration::from_secs(12)),
            },
            EndpointError::Unavailable {
                detail: "down".to_owned(),
                retry_after: None,
            },
            EndpointError::TimedOut {
                after: Duration::from_secs(30),
            },
            EndpointError::Internal("postgres://user:hunter2@db".to_owned()),
            EndpointError::Refused(Rejected::TooManyTokens {
                count: 40,
                limit: 32,
            }),
        ] {
            let there_and_back = endpoint_error_in(endpoint_error_out(&error));
            assert_eq!(
                there_and_back.to_string(),
                error.to_string(),
                "a failure a peer acts on differently must not become another one"
            );
            assert_eq!(there_and_back.retry_after(), error.retry_after());
        }
    }

    #[test]
    fn the_token_ceiling_crosses_with_its_fields_named() {
        // `token-ceiling` carries the seed's own field names — a record, not a positional
        // tuple — so what is worth pinning is that neither side swaps them: how many
        // arrived is `count`, and the ceiling it exceeded is `limit`.
        let refused = Rejected::TooManyTokens {
            count: 40,
            limit: 32,
        };
        assert!(matches!(
            rejected_out(&refused),
            wt::Rejected::TooManyTokens(wt::TokenCeiling {
                count: 40,
                limit: 32
            })
        ));
        assert_eq!(
            rejected_in(rejected_out(&refused)).to_string(),
            refused.to_string()
        );
    }

    #[test]
    fn a_hosts_own_finding_keeps_its_rule_id_in_the_prose() {
        // The one lossy direction in this file, and what it does instead of leaking or
        // inventing: the host's id is still in front of a reader, and the field says the
        // finding came from the host rather than from clause 5 as this engine applies it.
        let theirs = wt::Violation {
            severity: wt::Severity::Warning,
            path: "submessages[0]".to_owned(),
            rule: "their-engine/some-rule".to_owned(),
            detail: "the content is not a string".to_owned(),
        };

        let mine = violation_in(theirs);

        assert_eq!(mine.severity, Severity::Warning);
        assert_eq!(mine.path, "submessages[0]");
        assert_eq!(mine.rule, HOST_VIOLATION);
        assert!(mine.detail.contains("their-engine/some-rule"));
        assert!(mine.detail.contains("the content is not a string"));
    }

    #[test]
    fn clause_sixs_configuration_survives_the_round_trip() {
        let config = ConformityConfig::new()
            .with_conversation_token(true)
            .with_authentication_token(true)
            .with_max_tokens_per_message(7);
        assert_eq!(config_in(config_out(&config)), config);

        let rejection = Rejection::new(
            Policy::new(Disposition::Ignore, Disposition::Reject),
            Policy::new(Disposition::Log, Disposition::Log),
        );
        assert_eq!(rejection_in(rejection_out(&rejection)), rejection);
    }

    #[test]
    fn what_an_outcome_owes_is_asked_of_the_engine_and_not_restated() {
        // `outcome_for_rules` is sound for a rule that reads the variant, and `owed` is
        // one — including for `malformed`, whose payload it never looks at.
        let not_json = Message::from_json_bytes(b"{").expect_err("not a document");
        let malformed = wt::Outcome::Malformed(parse_error_out(&not_json));
        assert_eq!(
            nlipsy_core::owed(&outcome_for_rules(malformed)),
            Owed::Nothing
        );

        let ceiling = wt::Outcome::Refused(rejected_out(&Rejected::TooManyTokens {
            count: 40,
            limit: 32,
        }));
        assert_eq!(
            nlipsy_core::owed(&outcome_for_rules(ceiling)),
            Owed::ControlOnly
        );

        assert_eq!(
            nlipsy_core::owed(&outcome_for_rules(wt::Outcome::Ok)),
            Owed::Clause6
        );
    }

    #[test]
    fn a_reply_carries_the_four_views_the_wrapper_would_otherwise_re_derive() {
        let mut message = Message::error("no");
        message.submessages = vec![
            Submessage::token("conversation_9.2.3.5", "c-1").with_label("seq-1"),
            Submessage::token("authentication_9.2.3.5", "a-1"),
            Submessage::text("because"),
        ];
        let reply = Reply::new(
            ExchangeDetail::Http {
                status: 503,
                retry_after: Some(Duration::from_secs(12)),
            },
            42,
            Body::Message(message),
        );

        let crossed = reply_out(&reply);

        assert_eq!(crossed.bytes, 42);
        // Every `text` content in document order, joined by single spaces — the error
        // message's own first submessage first, then the `submessages` array (§5.1.5).
        assert_eq!(crossed.text.as_deref(), Some("no because"));
        assert_eq!(crossed.conversation_tokens, ["c-1"]);
        assert_eq!(crossed.authentication_tokens, ["a-1"]);
        assert!(crossed.is_error, "`MessageType: Error` is what this says");
        assert!(matches!(
            crossed.detail,
            wt::ExchangeDetail::Http(wt::HttpDetail {
                status: 503,
                retry_after_ms: Some(12_000)
            })
        ));

        // The whole submessage, subformat and label included: §6.2 ¶2 asks for "the
        // exact same format, subformat and content", and §6.2.1 gives the suffix after
        // `_` the identity of the agent that created the conversation. A caller holding
        // only the two string views above could rebuild neither token, which is why the
        // list is a field of its own. Read back through `submessage_in`, because the
        // generated records carry no `PartialEq` and the engine's own type does.
        let returned: Vec<Submessage> = crossed.tokens.into_iter().map(submessage_in).collect();
        assert_eq!(
            returned,
            vec![
                Submessage::token("conversation_9.2.3.5", "c-1").with_label("seq-1"),
                Submessage::token("authentication_9.2.3.5", "a-1"),
            ]
        );
    }

    #[test]
    fn a_body_that_is_not_a_message_crosses_as_the_preview_and_the_reason() {
        let reply = Reply::new(
            ExchangeDetail::Http {
                status: 502,
                retry_after: None,
            },
            9,
            Body::Opaque {
                text: "<html>gateway</html>".to_owned(),
                why: "body is not valid JSON".to_owned(),
            },
        );

        let crossed = reply_out(&reply);

        assert!(matches!(crossed.body, wt::Body::Opaque(_)));
        // A caller must not be left believing an end-point that never spoke NLIP said
        // something in it.
        assert!(crossed.is_error);
        assert_eq!(crossed.text.as_deref(), Some("<html>gateway</html>"));
    }

    #[test]
    fn a_transport_failure_names_where_the_request_was_going() {
        for error in [
            TransportError::Unreachable {
                endpoint: "https://example.test/nlip".to_owned(),
                detail: "connection refused".to_owned(),
            },
            TransportError::Interrupted {
                endpoint: "https://example.test/nlip".to_owned(),
                detail: "the connection broke".to_owned(),
            },
            TransportError::Malformed {
                detail: "not a response".to_owned(),
            },
        ] {
            assert_eq!(
                transport_error_in(transport_error_out(&error)).to_string(),
                error.to_string(),
                "the three differ in what a caller may safely do next"
            );
        }
    }

    #[test]
    fn an_exchange_crosses_with_its_status_and_its_delay() {
        let exchange = Exchange::new(
            b"{}".to_vec(),
            ExchangeDetail::Http {
                status: 429,
                retry_after: Some(Duration::from_millis(1500)),
            },
        );
        let crossed = exchange_in(wt::Exchange {
            body: exchange.body.clone(),
            detail: exchange_detail_out(&exchange.detail),
        });
        assert_eq!(crossed, exchange);
    }
}
