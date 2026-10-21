//! The corpus's second case kind: one clause-6 exchange, as data.
//!
//! A message case (`crate::Case`) is bytes in, decode-and-validate out. It can say
//! everything about what a message *is* — ECMA-430 clause 5 — and nothing about what an
//! end-point must *do* with one, because clause 6 is about a pair of messages and a
//! memory of what this end-point issued. An exchange case supplies both: a request, a
//! scripted end-point (its clause-6 configuration, its policy, and the three answers a
//! [`TokenAuthority`] gives), the reply that end-point's handler produced, and the
//! properties the *finished* reply must have.
//!
//! Nothing here needs an executor. [`Conformity::check`] and [`Conformity::finish`] are
//! synchronous functions over values, so a case is driven by calling them in order with
//! no endpoint underneath — `Conformity<(), _>`, the same shape the core's own tests
//! use. That is what makes the file portable: an implementation in another language runs
//! the same two steps over the same JSON and compares the same properties, without
//! having to reproduce this crate's task model or its trait objects.
//!
//! `conformance/README.md` is the schema; this module is the Rust reading of it.

use std::collections::BTreeMap;
use std::sync::Mutex;

use nlipsy_core::{
    Conformity, ConformityConfig, Context, Disposition, Finding, Findings, Policy, Rejected,
    Rejection, TokenAuthority, TokenKind,
};
use nlipsy_protocol::{Message, Submessage};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Differences, Outcome, canonical_content, canonical_model, compact, present};

// ---------------------------------------------------------------------------
// The case format — conformance/README.md, "An exchange case"
//
// `deny_unknown_fields` everywhere, for the reason the message cases have it: a key
// the harness does not know is a key the harness is silently not checking.
// ---------------------------------------------------------------------------

/// One exchange case: a request, a scripted end-point, and what clause 6 must produce.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeCase {
    /// Always `exchange`. A message case spells `message` or omits the key.
    pub kind: String,
    /// One line, what the case is about.
    pub title: String,
    /// The sentence of the standard the case is testing.
    pub provenance: Provenance,
    /// The end-point the request is put to.
    #[serde(default)]
    pub endpoint: EndpointScript,
    /// The peer's message, as a JSON object.
    pub request: Value,
    /// What the exchange must produce.
    pub expect: ExchangeExpect,
    /// A doubt about the expectation, a conflict with the standard, or a defensible
    /// alternative reading.
    pub notes: Option<String>,
}

/// Which sentence of which standard an exchange case is about.
///
/// Deliberately not the message cases' `source`. That block answers "where did these
/// bytes come from", and its `verbatim` / `composed` distinction is what tells evidence
/// about the protocol from evidence about an implementation's opinion. An exchange case
/// has no recorded bytes to be honest about — clause 6 prints no worked exchange
/// anywhere in ECMA-430 — so every request here is composed, and the thing worth
/// recording instead is the rule: the clause, and the sentence of it the expectations
/// are read out of.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// `ECMA-430` or `ECMA-431`.
    pub standard: String,
    /// The clause, as `6.2`, `6.2.1`, `6.3`, `6.4`.
    pub clause: String,
    /// The sentence the expectations are read out of, quoted.
    pub quote: String,
}

/// The end-point the request is put to, as data.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EndpointScript {
    /// §6.2's ceiling and §6.2.1/§6.2.2's two issuance switches.
    pub config: EndpointConfig,
    /// What a clause-5 finding does, per direction. Absent means the default: reject on
    /// error, log on warning, both ways.
    pub rejection: RejectionScript,
    /// The three answers a [`TokenAuthority`] gives.
    pub authority: AuthorityScript,
    /// The message the end-point's handler produced, as a JSON object.
    ///
    /// Absent on a case whose outcome is `refused`: the refusal happens in `check`,
    /// before the handler is asked anything, so there is no reply for the case to
    /// invent. On a [`short_circuit`](EndpointScript::short_circuit) case it is not the
    /// handler's at all — see there.
    pub reply: Option<Value>,
    /// [`Self::reply`] is what a *layer* answered, without the handler and without
    /// [`Conformity::finish`].
    ///
    /// A message-level layer — a cache, a canned answer, a maintenance-mode layer — may
    /// answer `Ok` without calling the rest of the stack, and the composer still runs
    /// [`Conformity::close`] over what it produced (`crates/nlipsy-core/src/serve.rs`,
    /// `serve_answered`). So §6.2's echo, §6.2.1's confirmation of what is already
    /// issued and §6.3's carry hold on the wire whoever produced the reply, while the
    /// outbound sanitiser and §6.2.1's minting — both of them `finish`'s — do not run.
    ///
    /// The standard knows no layers, so every case that sets this is a `readings/` case
    /// whose `notes` state the reading: the obligation attaches to the reply on the
    /// wire, not to the code path that produced it.
    ///
    /// `expect.findings` on such a case is therefore the **inbound** findings alone, and
    /// `expect.withheld` must be absent or `false`: nothing ran that could have withheld
    /// anything.
    pub short_circuit: bool,
}

/// `ConformityConfig` as a case spells it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EndpointConfig {
    /// §6.2.1 — issue a `conversation` token and keep confirming it.
    pub issue_conversation_token: bool,
    /// §6.2.2 — issue an `authentication` token.
    pub issue_authentication_token: bool,
    /// §6.2's ceiling. Defaults to 32, as the crate does.
    pub max_tokens_per_message: usize,
}

impl Default for EndpointConfig {
    fn default() -> EndpointConfig {
        let config = ConformityConfig::default();
        EndpointConfig {
            issue_conversation_token: config.issue_conversation_token,
            issue_authentication_token: config.issue_authentication_token,
            max_tokens_per_message: config.max_tokens_per_message,
        }
    }
}

/// `Rejection` as a case spells it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RejectionScript {
    /// Applied to the peer's request.
    pub inbound: PolicyScript,
    /// Applied to the end-point's own reply.
    pub outbound: PolicyScript,
}

/// One direction's policy: what an `error` and a `warning` each do.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PolicyScript {
    /// `reject`, `log` or `ignore`. Default `reject`.
    pub on_error: String,
    /// `reject`, `log` or `ignore`. Default `log`.
    pub on_warning: String,
}

impl Default for PolicyScript {
    fn default() -> PolicyScript {
        PolicyScript {
            on_error: "reject".to_owned(),
            on_warning: "log".to_owned(),
        }
    }
}

/// The three answers a [`TokenAuthority`] gives, as a script.
///
/// `own` is the set `is_own` answers `true` for — §6.2's exception, "when the token was
/// initially created by the end-point". `issued` is what `issued(kind)` returns, the
/// §6.2.1 confirmation an end-point owes on every reply. `mint` is what `mint(kind)`
/// returns, the one-time creation.
///
/// The three are independent on purpose, so that a case can put them in the
/// configurations that actually catch bugs: a token that is `own` but no longer
/// `issued` is what a peer holds after the conversation behind it was evicted, and an
/// end-point that echoed it as the peer's *and* minted a replacement would be wrong
/// twice. A value returned by `mint` becomes `own` and `issued` from that moment,
/// because from that moment the end-point did create it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthorityScript {
    /// Token contents this end-point created.
    pub own: Vec<String>,
    /// Per kind (`conversation`, `authentication`), what `issued` returns.
    pub issued: BTreeMap<String, String>,
    /// Per kind, what `mint` returns. A kind absent here mints nothing.
    pub mint: BTreeMap<String, String>,
}

/// What the exchange must produce.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeExpect {
    /// `reply` — the end-point answers — or `refused`, when `check` refuses the request
    /// before the handler runs.
    pub outcome: String,
    /// The refusal, when `outcome` is `refused`.
    pub refused: Option<ExpectedRefusal>,
    /// Properties of the finished reply, when `outcome` is `reply`.
    pub reply: Option<ExpectedReply>,
    /// `true` when clause 5 rejected the end-point's own answer and the reply is the
    /// replacement rather than the answer — `Finished::withheld`.
    ///
    /// Absent asserts nothing, which is what almost every case says about it: the flag
    /// is not a property of the *message* and a case that is about a clause of ECMA-430
    /// has no business pinning it. It is here because a binding reads it and nothing
    /// else in the corpus does — it is what `StatusMap::of` turns into a status line,
    /// so an implementation that always answered `false` would report a reply the peer
    /// never asked for as a success.
    ///
    /// Not derivable from `expect.reply`: a replacement error message and a handler that
    /// answered with `Message::error(..)` are the same bytes on the wire.
    pub withheld: Option<bool>,
    /// Every clause-5 finding the exchange reported, in both directions. Compared as a
    /// multiset; an absent key asserts nothing, an empty list asserts a clean exchange.
    pub findings: Option<Vec<ExpectedFinding>>,
}

/// A request clause 6 will not answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRefusal {
    /// `violations` or `too-many-tokens`.
    pub kind: String,
    /// For `violations`: the rule ids that caused it, as a set.
    pub rules: Option<Vec<String>>,
    /// For `too-many-tokens`: how many the message carried, the first submessage
    /// included.
    pub count: Option<usize>,
    /// For `too-many-tokens`: the ceiling it exceeded.
    pub limit: Option<usize>,
}

/// Properties of the finished reply.
///
/// Every field is optional, and each asserts one thing: a case says what its clause is
/// about and stays silent about the rest, so that a second implementation is not failed
/// for a choice the standard leaves it. [`ExpectedReply::canonical`] is the exception —
/// it pins the whole message, order included — and is used only where the order *is*
/// the reading being recorded.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReply {
    /// §5.1.1's `messagetype`, exactly. Explicit `null` asserts the field is absent.
    #[serde(default, deserialize_with = "present")]
    pub messagetype: Option<Option<String>>,
    /// §6.3's boolean. Explicit `null` asserts the field is absent.
    #[serde(default, deserialize_with = "present")]
    pub control: Option<Option<bool>>,
    /// The reply's own first submessage — §5.1.4's content, the end-point's answer.
    pub first_submessage: Option<ExpectedPart>,
    /// The `token` submessages of the reply's `submessages` array. See
    /// `conformance/README.md` for why this is a multiset and not a sequence.
    pub tokens: Option<Vec<ExpectedToken>>,
    /// Submessages that must be present, by format and subformat. Not exhaustive: it
    /// says "the reply carries this", not "the reply carries only this".
    pub contains: Option<Vec<ExpectedPart>>,
    /// The whole finished message in the corpus's canonical model spelling, order
    /// included.
    pub canonical: Option<Value>,
}

/// A submessage named by format and subformat.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedPart {
    /// The lower-case Table 1 value.
    pub format: String,
    /// The subformat, with the casing it must keep.
    pub subformat: String,
}

/// One token submessage of the reply, compared in full.
///
/// §6.2 asks for "the exact same format, subformat and content", so all three are
/// required. `label` is optional because ECMA-430 does not define one: an absent key
/// asserts nothing about it, and an explicit `null` asserts there is none.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedToken {
    /// Always `token`; written out so the file says what it is comparing.
    pub format: String,
    /// The subformat, byte for byte.
    pub subformat: String,
    /// The content, in the canonical model spelling.
    pub content: Value,
    /// The label, when the case is about one. Explicit `null` asserts there is none.
    #[serde(default, deserialize_with = "present")]
    pub label: Option<Option<String>>,
}

/// One clause-5 finding the exchange reported, and which direction reported it.
#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct ExpectedFinding {
    /// `inbound` — reported about the peer's request — or `outbound`, about the
    /// end-point's own reply.
    pub direction: String,
    /// The stable rule id, as `crate::ExpectedViolation` uses it.
    pub rule: String,
    /// `error` or `warning`.
    pub severity: String,
    /// Where in the message the finding is about.
    pub path: String,
}

// ---------------------------------------------------------------------------
// The scripted end-point
// ---------------------------------------------------------------------------

/// A [`TokenAuthority`] that answers out of the case file.
///
/// A deployment's real authority is a session table and a signature; this is the part of
/// one a case can state as data. `is_own` is a set membership, `issued` a lookup, `mint`
/// a lookup that remembers — and remembering is what makes the script coherent rather
/// than merely convenient: a value this authority has minted is thereafter one the
/// end-point created, which is exactly what §6.2's exception and §6.2.1's confirmation
/// both turn on.
#[derive(Debug)]
pub struct ScriptedAuthority {
    own: Vec<String>,
    issued: Minted,
    mint: Minted,
    /// What `mint` has handed out during this exchange.
    handed_out: Mutex<Minted>,
}

#[derive(Debug, Default, Clone)]
struct Minted {
    conversation: Option<String>,
    authentication: Option<String>,
}

impl Minted {
    fn from_map(map: &BTreeMap<String, String>) -> Minted {
        Minted {
            conversation: map.get("conversation").cloned(),
            authentication: map.get("authentication").cloned(),
        }
    }

    fn of(&self, kind: TokenKind) -> Option<String> {
        match kind {
            TokenKind::Conversation => self.conversation.clone(),
            TokenKind::Authentication => self.authentication.clone(),
        }
    }

    fn set(&mut self, kind: TokenKind, value: String) {
        match kind {
            TokenKind::Conversation => self.conversation = Some(value),
            TokenKind::Authentication => self.authentication = Some(value),
        }
    }

    fn contains(&self, token: &str) -> bool {
        self.conversation.as_deref() == Some(token) || self.authentication.as_deref() == Some(token)
    }
}

impl ScriptedAuthority {
    /// The authority a case scripts, for whichever implementation is being run: the
    /// Rust `Conformity` below, or the component's `token-authority` import in
    /// `crate::component`. One scripted authority for both is what makes the two runs
    /// comparable.
    pub fn new(script: &AuthorityScript) -> ScriptedAuthority {
        ScriptedAuthority {
            own: script.own.clone(),
            issued: Minted::from_map(&script.issued),
            mint: Minted::from_map(&script.mint),
            handed_out: Mutex::new(Minted::default()),
        }
    }
}

impl TokenAuthority for ScriptedAuthority {
    fn is_own(&self, token: &str, _cx: &Context) -> bool {
        self.own.iter().any(|own| own == token)
            || self
                .handed_out
                .lock()
                .expect("the scripted authority is not shared across a panic")
                .contains(token)
    }

    fn issued(&self, kind: TokenKind, _cx: &Context) -> Option<String> {
        self.handed_out
            .lock()
            .expect("the scripted authority is not shared across a panic")
            .of(kind)
            .or_else(|| self.issued.of(kind))
    }

    fn mint(&self, kind: TokenKind, _cx: &Context) -> Option<String> {
        let fresh = self.mint.of(kind)?;
        self.handed_out
            .lock()
            .expect("the scripted authority is not shared across a panic")
            .set(kind, fresh.clone());
        Some(fresh)
    }
}

// ---------------------------------------------------------------------------
// Running one exchange case
// ---------------------------------------------------------------------------

/// Run one exchange case against this workspace's clause 6.
///
/// Never [`Outcome::Skipped`]: clause 6 is about messages and not about wires, so an
/// exchange case runs in every build — including the one with no `cbor` feature, where
/// the ECMA-432 §7.1 message cases cannot.
pub fn run_exchange_case(case: &ExchangeCase) -> Outcome {
    let mut out = Differences::new();

    if case.kind != "exchange" {
        return Outcome::Failed(vec![format!("unknown case kind `{}`", case.kind)]);
    }
    if let Some(why) = malformed_case(case) {
        return Outcome::Failed(vec![why]);
    }

    let request = match Message::from_value(case.request.clone()) {
        Ok(request) => request,
        Err(why) => {
            return Outcome::Failed(vec![format!("request does not decode: {why}")]);
        }
    };

    let rejection = match rejection_of(&case.endpoint.rejection) {
        Ok(rejection) => rejection,
        Err(why) => return Outcome::Failed(vec![why]),
    };
    let config = ConformityConfig::new()
        .with_conversation_token(case.endpoint.config.issue_conversation_token)
        .with_authentication_token(case.endpoint.config.issue_authentication_token)
        .with_max_tokens_per_message(case.endpoint.config.max_tokens_per_message);

    // No endpoint underneath: `check` and `finish` are the whole of clause 6 and neither
    // asks the wrapper for one. `()` is what the core's own tests pass for the same
    // reason.
    let layer = Conformity::new(
        (),
        ScriptedAuthority::new(&case.endpoint.authority),
        config,
        rejection,
    );

    // An empty context. Clause 6 consults neither the peer nor the correlator — the two
    // things a `Context` carries by name — so a case has nothing to say about it, and a
    // field in the schema that changed no outcome would be a field a reader trusts.
    let cx = Context::new();

    let mut findings: Vec<Value> = Vec::new();

    match layer.check(&request, &cx) {
        Err(rejected) => {
            if case.expect.outcome != "refused" {
                out.push(format!(
                    "outcome: expected `{}`, the request was refused: {rejected}",
                    case.expect.outcome
                ));
            }
            compare_refusal(case, &ObservedRefusal::of(&rejected), &mut out);
        }
        Ok(inbound) => {
            if case.expect.outcome != "reply" {
                out.push(format!(
                    "outcome: expected `{}`, the request was accepted",
                    case.expect.outcome
                ));
            }
            collect(&mut findings, "inbound", &inbound);

            let Some(reply) = &case.endpoint.reply else {
                out.push("expect.outcome is `reply` with no `endpoint.reply`".into());
                return Outcome::Failed(out);
            };
            let reply = match Message::from_value(reply.clone()) {
                Ok(reply) => reply,
                Err(why) => {
                    out.push(format!("endpoint.reply does not decode: {why}"));
                    return Outcome::Failed(out);
                }
            };

            if case.endpoint.short_circuit {
                // Step 4 of the README's algorithm as a `short_circuit` case rewrites
                // it: `endpoint.reply` is what a layer answered, and only the three
                // idempotent obligations run over it. No sanitiser and no issuance —
                // `Conformity::close` is exactly those three — so there is no outbound
                // step to report a finding, and `withheld` is what a reply nothing
                // withheld says.
                let mut reply = reply;
                layer.close(&request, &mut reply, &cx);
                compare_reply(case, &reply, &mut out);
                compare_withheld(case, false, &mut out);
            } else {
                let finished = layer.finish(&request, reply, &cx);
                collect(&mut findings, "outbound", &finished.findings);
                compare_reply(case, &finished.message, &mut out);
                compare_withheld(case, finished.withheld(), &mut out);
            }
        }
    }

    compare_findings(case, &findings, &mut out);

    if out.is_empty() {
        Outcome::Passed
    } else {
        Outcome::Failed(out)
    }
}

/// What is wrong with the case *file*, when something is.
///
/// One rule today, and it belongs here rather than in either runner: a `short_circuit`
/// case may not expect `withheld: true`. Nothing on that path can withhold anything —
/// the outbound sanitiser is `finish`'s and `finish` never ran — so a case saying so is
/// asserting something no implementation can produce, and a corpus that let it through
/// would be failing every implementation for a typo. `deny_unknown_fields` catches the
/// keys a harness does not know; this catches the combinations it does.
pub fn malformed_case(case: &ExchangeCase) -> Option<String> {
    if case.endpoint.short_circuit && case.expect.withheld == Some(true) {
        return Some(
            "a `short_circuit` case cannot expect `withheld: true`: `finish` never ran, \
             so nothing sanitised the reply and nothing could have withheld it"
                .to_owned(),
        );
    }
    None
}

/// Compare the withholding flag with what the case expects of it, when it says anything.
pub fn compare_withheld(case: &ExchangeCase, actual: bool, out: &mut Differences) {
    if let Some(expected) = case.expect.withheld {
        if actual != expected {
            out.push(format!("withheld: expected {expected}, got {actual}"));
        }
    }
}

/// A case's clause-5 policy, as the engine's value.
pub fn rejection_of(script: &RejectionScript) -> Result<Rejection, String> {
    Ok(Rejection::new(
        policy_of(&script.inbound)?,
        policy_of(&script.outbound)?,
    ))
}

fn policy_of(script: &PolicyScript) -> Result<Policy, String> {
    Ok(Policy::new(
        disposition_of(&script.on_error)?,
        disposition_of(&script.on_warning)?,
    ))
}

fn disposition_of(name: &str) -> Result<Disposition, String> {
    match name {
        "reject" => Ok(Disposition::Reject),
        "log" => Ok(Disposition::Log),
        "ignore" => Ok(Disposition::Ignore),
        other => Err(format!(
            "unknown disposition `{other}`; expected reject, log or ignore"
        )),
    }
}

/// Add one step's findings to the exchange's list, tagged with the direction that
/// reported them.
///
/// `inbound` is what `check` returned and `outbound` is what `finish` returned, which is
/// the only way to attribute a finding to a direction — `served.findings` is one list
/// with no marked boundary (`wit/README.md` §4).
pub fn collect(into: &mut Vec<Value>, direction: &str, findings: &Findings) {
    into.extend(findings.iter().map(|finding: &Finding| {
        json!({
            "direction": direction,
            "rule": finding.violation.rule,
            "severity": finding.violation.severity.to_string(),
            "path": finding.violation.path,
        })
    }));
}

/// A refusal as the corpus describes one, whoever produced it.
///
/// The comparison is written against this rather than against [`Rejected`], for the
/// reason [`Observed`](crate::Observed) exists on the message side: the `component` mode
/// reports a refusal across a boundary that has no [`Rejected`] on it, and a rule id
/// that arrived as a `String` cannot be put back into a `Violation`, whose `rule` is a
/// `&'static str`.
#[derive(Debug)]
pub struct ObservedRefusal {
    /// `violations`, `too-many-tokens`, or `unmapped` for a case this corpus cannot
    /// express.
    pub kind: &'static str,
    /// For `violations`: the rule ids that caused it.
    pub rules: Vec<String>,
    /// For `too-many-tokens`: how many the message carried.
    pub count: Option<usize>,
    /// For `too-many-tokens`: the ceiling it exceeded.
    pub limit: Option<usize>,
}

impl ObservedRefusal {
    /// What this workspace produces.
    pub fn of(rejected: &Rejected) -> ObservedRefusal {
        match rejected {
            Rejected::Violations(violations) => ObservedRefusal {
                kind: "violations",
                rules: violations.iter().map(|one| one.rule.to_owned()).collect(),
                count: None,
                limit: None,
            },
            Rejected::TooManyTokens { count, limit } => ObservedRefusal {
                kind: "too-many-tokens",
                rules: Vec::new(),
                count: Some(*count),
                limit: Some(*limit),
            },
            // `Rejected` is `#[non_exhaustive]`: a guard added since is a case the
            // corpus cannot be expressing, and is reported rather than silently
            // accepted.
            _ => ObservedRefusal {
                kind: "unmapped",
                rules: Vec::new(),
                count: None,
                limit: None,
            },
        }
    }
}

/// Compare a refusal with what the case expects of one.
pub fn compare_refusal(case: &ExchangeCase, refused: &ObservedRefusal, out: &mut Differences) {
    let Some(expected) = &case.expect.refused else {
        if case.expect.outcome == "refused" {
            out.push("expect.outcome is `refused` with no `refused` object".into());
        }
        return;
    };

    if refused.kind != expected.kind {
        out.push(format!(
            "refused.kind: expected `{}`, got `{}`",
            expected.kind, refused.kind
        ));
    }

    if let Some(rules) = &expected.rules {
        let mut actual: Vec<&str> = refused.rules.iter().map(String::as_str).collect();
        let mut wanted: Vec<&str> = rules.iter().map(String::as_str).collect();
        actual.sort_unstable();
        wanted.sort_unstable();
        if actual != wanted {
            out.push(format!(
                "refused.rules: expected {wanted:?}, got {actual:?}"
            ));
        }
    }

    if let (Some(expected), Some(actual)) = (expected.count, refused.count) {
        if expected != actual {
            out.push(format!("refused.count: expected {expected}, got {actual}"));
        }
    }
    if let (Some(expected), Some(actual)) = (expected.limit, refused.limit) {
        if expected != actual {
            out.push(format!("refused.limit: expected {expected}, got {actual}"));
        }
    }
}

/// Compare a finished reply with the properties the case says clause 6 obliges it to
/// have.
///
/// Every field is optional: a case says what its clause is about and stays silent about
/// the rest, so that a second implementation is not failed for a choice the standard
/// leaves it.
pub fn compare_reply(case: &ExchangeCase, reply: &Message, out: &mut Differences) {
    let Some(expected) = &case.expect.reply else {
        if case.expect.outcome == "reply" {
            out.push("expect.outcome is `reply` with no `reply` object".into());
        }
        return;
    };

    if let Some(wanted) = &expected.messagetype {
        let actual = reply.message_type.as_deref();
        if actual != wanted.as_deref() {
            out.push(format!(
                "reply.messagetype: expected {wanted:?}, got {actual:?}"
            ));
        }
    }

    if let Some(wanted) = &expected.control {
        if reply.control != *wanted {
            out.push(format!(
                "reply.control: expected {wanted:?}, got {:?}",
                reply.control
            ));
        }
    }

    if let Some(wanted) = &expected.first_submessage {
        let actual = ExpectedPart {
            format: reply.format.as_str().to_owned(),
            subformat: reply.subformat.clone(),
        };
        if actual.format != wanted.format || actual.subformat != wanted.subformat {
            out.push(format!(
                "reply.first_submessage: expected {}/{}, got {}/{}",
                wanted.format, wanted.subformat, actual.format, actual.subformat
            ));
        }
    }

    if let Some(wanted) = &expected.tokens {
        compare_tokens(wanted, reply, out);
    }

    if let Some(wanted) = &expected.contains {
        for part in wanted {
            let present = reply.submessages.iter().any(|submessage| {
                submessage.format.as_str() == part.format && submessage.subformat == part.subformat
            });
            if !present {
                out.push(format!(
                    "reply.contains: no {}/{} submessage in {}",
                    part.format,
                    part.subformat,
                    compact(&canonical_model(reply))
                ));
            }
        }
    }

    if let Some(wanted) = &expected.canonical {
        let actual = canonical_model(reply);
        if &actual != wanted {
            out.push(format!(
                "reply.canonical: expected {}, got {}",
                compact(wanted),
                compact(&actual)
            ));
        }
    }
}

/// §6.2's echo, compared as a **multiset**.
///
/// The clause says what must come back — "the same submessage with the exact same
/// format, subformat and content" — and says nothing about where in the reply it goes.
/// Asserting the order would fail a conforming implementation for a choice the standard
/// leaves it: this crate puts the request's first-submessage token first because that is
/// where the peer put it and §5.1.5 makes the order of submessages significant, but a
/// JavaScript implementation that appends in a different order has broken no rule.
/// `readings/48` is the case that pins the order *this* implementation takes, and it
/// does so through `canonical`, where order is what is being recorded.
///
/// Matched by removal rather than by sorting two lists, because an expectation may be
/// silent about a token's `label` and a sorted comparison has no way to express "any".
fn compare_tokens(wanted: &[ExpectedToken], reply: &Message, out: &mut Differences) {
    let mut unmatched: Vec<&Submessage> = reply.tokens().collect();
    for expected in wanted {
        match unmatched
            .iter()
            .position(|token| token_matches(expected, token))
        {
            Some(index) => {
                unmatched.remove(index);
            }
            None => out.push(format!(
                "reply.tokens: expected but not returned: {}",
                expected_token_key(expected)
            )),
        }
    }
    for extra in unmatched {
        out.push(format!(
            "reply.tokens: returned but not expected: {}",
            token_key(extra)
        ));
    }
}

fn token_matches(expected: &ExpectedToken, token: &Submessage) -> bool {
    let label_matches = match &expected.label {
        // An absent `label` key asserts nothing about one.
        None => true,
        Some(wanted) => token.label.as_deref() == wanted.as_deref(),
    };
    label_matches
        && token.format.as_str() == expected.format
        && token.subformat == expected.subformat
        && canonical_content(&token.content) == expected.content
}

/// One token as a comparable line, so a difference reads as text rather than as a
/// structure a reader has to line up by eye.
fn token_key(token: &Submessage) -> String {
    let label = match &token.label {
        Some(label) => format!(" label={label}"),
        None => String::new(),
    };
    format!(
        "{}/{} = {}{label}",
        token.format.as_str(),
        token.subformat,
        compact(&canonical_content(&token.content))
    )
}

fn expected_token_key(token: &ExpectedToken) -> String {
    let label = match &token.label {
        Some(Some(label)) => format!(" label={label}"),
        Some(None) => " label=none".to_owned(),
        None => String::new(),
    };
    format!(
        "{}/{} = {}{label}",
        token.format,
        token.subformat,
        compact(&token.content)
    )
}

/// Compare the clause-5 findings of both directions with the case's, as a multiset.
///
/// An absent `findings` key asserts nothing; an empty list asserts a clean exchange.
pub fn compare_findings(case: &ExchangeCase, actual: &[Value], out: &mut Differences) {
    let Some(expected) = &case.expect.findings else {
        return;
    };
    let mut wanted: Vec<String> = expected
        .iter()
        .map(|finding| {
            format!(
                "{} {} {} at {}",
                finding.direction, finding.severity, finding.rule, finding.path
            )
        })
        .collect();
    let mut actual: Vec<String> = actual
        .iter()
        .map(|finding| {
            format!(
                "{} {} {} at {}",
                finding["direction"].as_str().unwrap_or_default(),
                finding["severity"].as_str().unwrap_or_default(),
                finding["rule"].as_str().unwrap_or_default(),
                finding["path"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    wanted.sort();
    actual.sort();
    if wanted != actual {
        out.push(format!("findings: expected {wanted:?}, got {actual:?}"));
    }
}
