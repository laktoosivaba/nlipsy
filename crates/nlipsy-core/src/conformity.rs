//! The protocol's own obligations, applied around the endpoint — ECMA-430 clause 6.
//!
//! Everything here is required by the standard and has nothing to do with what an
//! endpoint is *for*. It lives in one place, on the path between decoding and
//! encoding, so that business logic can be written naturally and still produce a
//! conforming exchange:
//!
//! * **§6.2 — token echo.** A message carrying `token` submessages must be answered
//!   with "the same submessage with the exact same format, subformat and content",
//!   except for tokens the endpoint itself created. This is the rule that lets two
//!   peers keep credentials and conversation identifiers in sync without either one
//!   having to understand the other's internals, and it is also the easiest one to
//!   lose track of — a reply built from scratch silently drops it. The message's own
//!   first submessage counts as one of those submessages (§5 ¶2), which is what
//!   [`first_submessage_token`] exists for.
//! * **§6.2.1, §6.2.2 — token issuance.** Optional, and delegated to a
//!   [`TokenAuthority`]: this crate decides *when* a token is owed and the deployment
//!   decides what one is.
//! * **§6.3 — control echo.** A control request gets a control answer, in whichever of
//!   the two spellings the peer used.
//!
//! It also refuses to send a reply that breaks clause 5, so a buggy endpoint degrades
//! into a protocol error rather than into a malformed reply a peer has to guess at.
//!
//! # Three synchronous steps, and one wrapper built from them
//!
//! [`Conformity::check`] runs before the endpoint, [`Conformity::finish`] after it,
//! and [`Conformity::echo_into`] is the one echo loop both that and a runtime's
//! failure path use. [`Conformity::close`] is the three idempotent obligations
//! together, for a binding to run over a reply `finish` never saw — a layer that
//! answered on its own, or a refusal built without a handler. None of them is `async`,
//! none of them allocates a task, and all of them are testable with no executor —
//! which is the test that decides what belongs in this crate at all. [`Conformity`]
//! itself implements [`Endpoint`] by composing them around an inner endpoint, and is a
//! convenience for Rust runtimes rather than the primitive.

use nlipsy_protocol::error::{Severity, Violation};
use nlipsy_protocol::message::{Message, Submessage};
use thiserror::Error;

use crate::context::Context;
use crate::endpoint::{Endpoint, EndpointError};
use crate::first_submessage::first_submessage_token;
use crate::token::{TokenAuthority, TokenKind};

/// How many `token` submessages a request carries, the first submessage included.
///
/// Counted rather than collected: this runs before the ceiling below is applied, so a
/// message with a hundred thousand tokens must not cost a hundred thousand clones to
/// refuse.
pub fn token_count(request: &Message) -> usize {
    request.tokens().count() + usize::from(request.is_token())
}

/// Default upper bound on token submessages echoed from a single request.
///
/// §6.2 obliges the endpoint to return *all* of them, so the ceiling is not a
/// truncation point: a request above it is refused outright, because answering it
/// would mean silently dropping tokens the peer is entitled to see. The number is a
/// deployment's choice and lives in [`ConformityConfig`]; 32 is what one working
/// endpoint chose.
///
/// The refusal is this engine's one deliberate departure from §6.2 ¶2, taken under
/// ECMA-434 §7.2.5 ("NLIP agents should implement throttling … to prevent flooding and
/// denial-of-service attacks") rather than under clause 6: the reply carries no echo,
/// because echoing what was refused is the work the ceiling exists to refuse. At the
/// default no conforming exchange meets it — §6.2 ¶3's example is two tokens per
/// end-point — so the departure is reachable only by a peer that is not having a
/// conversation.
pub const DEFAULT_MAX_TOKENS_PER_MESSAGE: usize = 32;

/// What an endpoint does about the optional parts of clause 6.
///
/// `#[non_exhaustive]`: §6 has more optional corners than these three and a fourth
/// switch must not be a breaking change. Build one with [`ConformityConfig::new`] and
/// the `with_*` shorthands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConformityConfig {
    /// Issue a `conversation` token and keep returning it (§6.2.1).
    pub issue_conversation_token: bool,
    /// Issue an `authentication` token on the first request (§6.2.2) — anonymous
    /// mode, as recommended by TR/113 §10.
    pub issue_authentication_token: bool,
    /// Upper bound on the token submessages one request may carry.
    ///
    /// See [`DEFAULT_MAX_TOKENS_PER_MESSAGE`] for why exceeding it refuses the request
    /// rather than truncating the echo.
    pub max_tokens_per_message: usize,
}

impl Default for ConformityConfig {
    /// Issue nothing, and refuse above [`DEFAULT_MAX_TOKENS_PER_MESSAGE`] tokens.
    ///
    /// Both issuance switches are off because §6.2's echo is the only *mandatory*
    /// half of the clause: an endpoint that has not been told to keep conversation
    /// state should behave like the reference implementation — echo what it was sent
    /// and create nothing of its own.
    fn default() -> ConformityConfig {
        ConformityConfig {
            issue_conversation_token: false,
            issue_authentication_token: false,
            max_tokens_per_message: DEFAULT_MAX_TOKENS_PER_MESSAGE,
        }
    }
}

impl ConformityConfig {
    /// The defaults: issue nothing, refuse above 32 tokens.
    pub fn new() -> ConformityConfig {
        ConformityConfig::default()
    }

    /// Issue a `conversation` token (§6.2.1).
    pub fn with_conversation_token(mut self, issue: bool) -> ConformityConfig {
        self.issue_conversation_token = issue;
        self
    }

    /// Issue an `authentication` token (§6.2.2).
    pub fn with_authentication_token(mut self, issue: bool) -> ConformityConfig {
        self.issue_authentication_token = issue;
        self
    }

    /// Change the token ceiling.
    pub fn with_max_tokens_per_message(mut self, limit: usize) -> ConformityConfig {
        self.max_tokens_per_message = limit;
        self
    }
}

/// What a clause-5 finding does to an exchange.
///
/// Exhaustive, like [`Severity`]: refuse, record, or ignore is the whole space, and a
/// consumer branching on it should keep being told when it has missed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Refuse the exchange. Inbound this is a rejected request; outbound it is a reply
    /// replaced by a protocol-legal error.
    Reject,
    /// Carry on, and hand the finding back so the runtime can record it.
    Log,
    /// Carry on, and say nothing.
    ///
    /// `Ignore` on an inbound [`Severity::Error`] hands the endpoint a message the
    /// model says cannot be interpreted as written — a `text` part whose content is
    /// not a string, an empty subformat — and returns no finding for it. Nothing the
    /// end-point owes the peer is broken; the peer's message was the non-conforming
    /// one. The endpoint must then not assume what clause 5 would have guaranteed:
    /// `content_as_str()` may be `None` on a `text` part, `subformat` may be empty.
    /// `Ignore` differs from [`Disposition::Log`] only in whether the finding is
    /// returned in [`Findings`]; this crate logs nothing either way. The echo is
    /// unaffected — a `token` part is cloned whole, whatever its content — and a reply
    /// that re-emits the offending content is judged by the outbound policy, whose
    /// default replaces it.
    Ignore,
}

/// What each [`Severity`] does, in one direction.
///
/// `#[non_exhaustive]`: a third severity would add a third field. Build one with
/// [`Policy::new`] or take [`Policy::default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Policy {
    /// What to do with a finding the message model calls fatal.
    ///
    /// Inbound, anything but [`Disposition::Reject`] admits a request clause 5 says
    /// cannot be interpreted as written — see [`Disposition::Ignore`] for what the
    /// endpoint must then stop assuming. Outbound, see [`Rejection::outbound`].
    pub on_error: Disposition,
    /// What to do with a finding the message model calls off-spec but interpretable.
    pub on_warning: Disposition,
}

impl Default for Policy {
    /// Reject on error, log on warning.
    fn default() -> Policy {
        Policy {
            on_error: Disposition::Reject,
            on_warning: Disposition::Log,
        }
    }
}

impl Policy {
    /// A policy from its two dispositions.
    pub fn new(on_error: Disposition, on_warning: Disposition) -> Policy {
        Policy {
            on_error,
            on_warning,
        }
    }

    /// The disposition this policy assigns to `severity`.
    pub fn of(&self, severity: Severity) -> Disposition {
        match severity {
            Severity::Error => self.on_error,
            Severity::Warning => self.on_warning,
        }
    }
}

/// What clause-5 findings do, in each direction.
///
/// Two policies rather than one, deliberately. An endpoint that tolerates a peer's
/// warnings should not therefore tolerate its own: the inbound policy is about a
/// message somebody else wrote, where leniency buys interoperability, and the outbound
/// one is about a message this endpoint wrote, where leniency only hides a bug.
///
/// `nlipsy-protocol` declines to make this choice — it reports a
/// [`Violation`]'s severity and says that "a deployment may choose to reject it or
/// merely to log it" — and this is where that hint is finally spent. Two deployments
/// want different values and both are legitimate: a gateway in front of untrusted
/// peers wants `on_warning: Reject`, the strictest reading of clause 5, while a bridge
/// relaying from an older implementation wants `on_error: Log`, because the standard
/// contradicts itself in places and a strict endpoint would refuse messages the
/// reference SDK emits.
///
/// `#[non_exhaustive]`: build one with [`Rejection::new`] or take the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Rejection {
    /// Applied to the peer's request, before the endpoint sees it.
    pub inbound: Policy,
    /// Applied to the endpoint's own reply, before it reaches the wire.
    ///
    /// `on_error` other than [`Disposition::Reject`] sends a reply the message model
    /// says cannot be interpreted as written; a deployment choosing it — a relay
    /// forwarding a peer's off-spec content unchanged — has taken clause 5 onto
    /// itself, and this crate no longer vouches for what goes on the wire.
    pub outbound: Policy,
}

impl Rejection {
    /// A rejection policy from its two directions.
    pub fn new(inbound: Policy, outbound: Policy) -> Rejection {
        Rejection { inbound, outbound }
    }
}

/// One clause-5 finding and what the policy decided about it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Finding {
    /// What the message model said.
    pub violation: Violation,
    /// What [`Rejection`] said to do about it.
    pub disposition: Disposition,
}

/// The clause-5 findings of one step, with their dispositions.
///
/// Returned as data rather than written to a log, because this crate has no logger and
/// should not acquire one: which findings are worth a line, and at what level, is the
/// runtime's decision. [`Conformity::check`] returns only findings it did *not* reject
/// — a rejection is the error instead — while [`Conformity::finish`] returns all of
/// them, rejected ones included, because a reply the endpoint itself got wrong is
/// something the operator has to be able to see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    findings: Vec<Finding>,
}

impl Findings {
    /// `true` when clause 5 had nothing to say.
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    /// How many findings there are.
    pub fn len(&self) -> usize {
        self.findings.len()
    }

    /// Every finding, in the order the message model reported it.
    pub fn iter(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter()
    }

    /// Only the findings whose disposition is [`Disposition::Log`] — the ones a
    /// runtime is being asked to record.
    pub fn to_log(&self) -> impl Iterator<Item = &Violation> {
        self.findings
            .iter()
            .filter(|finding| finding.disposition == Disposition::Log)
            .map(|finding| &finding.violation)
    }

    /// Only the findings that caused a rejection or a replacement.
    pub fn rejected(&self) -> impl Iterator<Item = &Violation> {
        self.findings
            .iter()
            .filter(|finding| finding.disposition == Disposition::Reject)
            .map(|finding| &finding.violation)
    }

    /// Append `other`'s findings after this one's.
    ///
    /// `pub(crate)` and not public: the only caller is
    /// [`serve`](crate::serve::serve), which concatenates the request's findings with
    /// the reply's in that order, and a consumer has nothing to build a [`Finding`]
    /// from — the type is `#[non_exhaustive]`.
    pub(crate) fn extend(&mut self, other: Findings) {
        self.findings.extend(other.findings);
    }

    fn push(&mut self, violation: Violation, disposition: Disposition) {
        self.findings.push(Finding {
            violation,
            disposition,
        });
    }
}

/// A request that cannot be answered conformingly.
///
/// `#[non_exhaustive]`: a later clause-6 guard adds a variant. Both arms describe the
/// *peer's* message rather than the endpoint's state, which is why
/// [`Display`](std::fmt::Display) is safe to return to the peer — it says nothing the
/// peer did not already send.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Rejected {
    /// The message broke clause 5 under a policy that rejects.
    ///
    /// Non-empty by construction, and in the order the model reported them.
    #[error("message is not conforming: {}{}", first_or_empty(.0), and_more(.0))]
    Violations(Vec<Violation>),

    /// More token submessages than the endpoint is willing to echo.
    #[error("message carries {count} token submessages; this endpoint echoes at most {limit}")]
    TooManyTokens {
        /// How many the message carried, the first submessage included.
        count: usize,
        /// The ceiling from [`ConformityConfig::max_tokens_per_message`].
        limit: usize,
    },
}

fn first_or_empty(violations: &[Violation]) -> String {
    match violations.first() {
        Some(violation) => violation.to_string(),
        None => String::new(),
    }
}

fn and_more(violations: &[Violation]) -> String {
    match violations.len() {
        0 | 1 => String::new(),
        more => format!(" (and {} further finding(s))", more - 1),
    }
}

/// A reply that has been through [`Conformity::finish`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Finished {
    /// What goes on the wire.
    pub message: Message,
    /// What clause 5 said about the endpoint's own reply, and what was done about it.
    pub findings: Findings,
}

impl Finished {
    /// `true` when the endpoint's own answer was rejected by clause 5 and
    /// [`Finished::message`] is the replacement rather than the answer.
    ///
    /// A binding with a status line to set wants this: the peer asked a question this
    /// end-point could not answer conformingly, which is the end-point failing, and
    /// reporting success over a message the peer never asked for tells a peer reading
    /// only the status line the opposite of what happened.
    pub fn withheld(&self) -> bool {
        self.findings.rejected().next().is_some()
    }
}

/// ECMA-430 clause 6, wrapped around an endpoint.
///
/// Holds the inner endpoint, the [`TokenAuthority`] that answers §6.2's exception, and
/// the two policy values. The three obligations are public methods so a runtime can
/// drive them itself — a binding that has to put a timeout around the handler alone,
/// or that answers a failure before the handler ran, needs the steps and not the
/// wrapper — and the [`Endpoint`] implementation composes them for everyone else.
#[derive(Debug, Clone)]
pub struct Conformity<E, A> {
    inner: E,
    authority: A,
    config: ConformityConfig,
    rejection: Rejection,
}

impl<E, A> Conformity<E, A> {
    /// Wrap `inner`, with `authority` answering §6.2's exception.
    pub fn new(
        inner: E,
        authority: A,
        config: ConformityConfig,
        rejection: Rejection,
    ) -> Conformity<E, A> {
        Conformity {
            inner,
            authority,
            config,
            rejection,
        }
    }

    /// The endpoint underneath.
    pub fn inner(&self) -> &E {
        &self.inner
    }

    /// The authority answering §6.2's exception.
    pub fn authority(&self) -> &A {
        &self.authority
    }

    /// The optional-features configuration.
    pub fn config(&self) -> &ConformityConfig {
        &self.config
    }

    /// The clause-5 policy, in both directions.
    pub fn rejection(&self) -> &Rejection {
        &self.rejection
    }
}

impl<E, A: TokenAuthority> Conformity<E, A> {
    /// Everything that must be settled before the endpoint is asked anything.
    ///
    /// Clause 5 first, under [`Rejection::inbound`], then §6.2's token ceiling. The
    /// order matters: a message that cannot be interpreted has no meaningful token
    /// count, and counting first would refuse it for the wrong reason.
    ///
    /// `cx` is unused today and is in the signature because the policy this applies is
    /// the kind that varies by peer — a gateway is stricter with strangers than with
    /// its own sidecars — and because the three steps read better with one shape.
    pub fn check(&self, request: &Message, cx: &Context) -> Result<Findings, Rejected> {
        let _ = cx;

        let mut findings = Findings::default();
        let mut rejected: Vec<Violation> = Vec::new();
        for violation in request.validate() {
            match self.rejection.inbound.of(violation.severity) {
                Disposition::Reject => rejected.push(violation),
                disposition => findings.push(violation, disposition),
            }
        }
        if !rejected.is_empty() {
            return Err(Rejected::Violations(rejected));
        }

        let count = token_count(request);
        if count > self.config.max_tokens_per_message {
            return Err(Rejected::TooManyTokens {
                count,
                limit: self.config.max_tokens_per_message,
            });
        }

        Ok(findings)
    }

    /// Turn an endpoint's answer into a conforming reply.
    ///
    /// Five steps, in this order and for this reason: the reply is sanitised against
    /// clause 5 *before* anything is added to it, so a replacement error message
    /// carries the echo rather than losing it; then §6.2's echo, then §6.2.1's
    /// confirmation of what is already issued, then §6.2.1/§6.2.2's minting, then
    /// §6.3's control carry — which overwrites the boolean in a control exchange,
    /// because the clause carries no exception, and fills `messagetype` only where the
    /// endpoint left it empty.
    ///
    /// Minting is here and nowhere else, which is why this is the step that runs at the
    /// bottom of a binding's layer stack: a deployment that persists what it minted has
    /// to run after it. Everything else this does is idempotent and is also reachable
    /// as [`Self::close`], for the paths `finish` never sees.
    pub fn finish(&self, request: &Message, reply: Message, cx: &Context) -> Finished {
        let (mut message, findings) = self.sanitise(request, reply);
        self.echo_into(request, &mut message, cx);
        self.confirm_issued(&mut message, cx);
        self.mint_if_needed(request, &mut message, cx);
        carry_control(request, &mut message);
        Finished { message, findings }
    }

    /// The obligations of clause 6 that can be applied twice, for a reply that may
    /// not have been through [`Self::finish`].
    ///
    /// §6.2's echo, §6.2.1's confirmation of what this endpoint has already issued,
    /// and §6.3's control carry. All three are idempotent — the echo skips a
    /// submessage the reply already carries, the confirmation goes through the same
    /// deduplication, and the carry is a set — so running this over a finished reply
    /// changes nothing, and running it over a reply `finish` never saw restores what
    /// the peer is owed.
    ///
    /// **This is what a binding calls after anything that could have bypassed or
    /// altered `finish`.** A message-level layer that answers `Ok(..)` without calling
    /// the rest of the stack — a cache, a canned answer, a maintenance-mode layer —
    /// never reaches `finish` at all, and a layer's after-half receives the finished
    /// reply by value and can strip a token from it. Neither is forbidden: they are
    /// the two useful things a layer does. A binding that promises the obligations
    /// cannot be skipped has to make that true afterwards rather than assume it, and
    /// the cost on the normal path is three linear scans of a reply that already
    /// satisfies all three.
    ///
    /// It deliberately does **not** mint (§6.2.1 ¶1, §6.2.2). Minting is a write, and
    /// a deployment that persists what was minted commits on the success path only, so
    /// a token created on a short circuit or a refusal would be handed to the peer and
    /// forgotten. Creation stays in `finish`, at the bottom of the stack.
    pub fn close(&self, request: &Message, reply: &mut Message, cx: &Context) {
        self.echo_into(request, reply, cx);
        self.confirm_issued(reply, cx);
        carry_control(request, reply);
    }

    /// §6.2 — put every token the peer sent into `reply`, except those this endpoint
    /// minted.
    ///
    /// The one echo loop. [`Self::finish`] calls it on the happy path and a runtime
    /// calls it directly on the failure path, where a request can fail before a
    /// session exists and its tokens are still owed back. Having two loops — one that
    /// consults the session and one that does not — is how the failure path ends up
    /// silently using the weaker test.
    ///
    /// "Except when the token was initially created by the end-point" is decided by
    /// [`TokenAuthority::is_own`]. A token the peer created — including one another
    /// agent created and the peer is relaying — is opaque here and comes straight
    /// back, unchanged, which is exactly what §6.2 asks for.
    ///
    /// Every echo lands in the reply's `submessages`, the first submessage of the
    /// request included. §6.2 ¶2 asks for "the same submessage with the exact same
    /// format, subformat and content" and says nothing about position, while the
    /// reply's *own* first submessage is §5.1.4's content — the endpoint's answer —
    /// and is not free to be a token. A peer whose whole message was a token therefore
    /// gets an answer with its token beside it, rather than an answer replaced by it.
    pub fn echo_into(&self, request: &Message, reply: &mut Message, cx: &Context) {
        // The first submessage leads, because that is where the peer put it and
        // §5.1.5 makes the order of submessages significant.
        let first = first_submessage_token(request);
        for token in first.iter().chain(request.tokens()) {
            let content = token.content_as_str().unwrap_or_default();
            if self.authority.is_own(content, cx) {
                continue;
            }
            if is_already_present(reply, token) {
                continue;
            }
            reply.submessages.push(token.clone());
        }
    }

    /// §6.2.1 — the tokens this endpoint has already issued, confirmed again.
    ///
    /// A token this endpoint already issued comes back on **every** reply, including
    /// the reply to the message that carried it — that is how the peer sees the
    /// conversation is still the same one. Dropping it would leave the peer holding an
    /// identifier the server no longer confirms. Nothing about what else the request
    /// carried changes that, which is why [`TokenAuthority::issued`] is consulted
    /// before anything is asked about the request, and why this half is in
    /// [`Self::close`] and the minting half is not.
    ///
    /// Idempotent: [`push_token`] adds nothing the reply already carries.
    fn confirm_issued(&self, reply: &mut Message, cx: &Context) {
        for (wanted, kind) in [
            (self.config.issue_conversation_token, TokenKind::Conversation),
            (
                self.config.issue_authentication_token,
                TokenKind::Authentication,
            ),
        ] {
            if !wanted {
                continue;
            }
            if let Some(token) = self.authority.issued(kind, cx) {
                push_token(reply, kind.subformat(), token);
            }
        }
    }

    /// §6.2.1/§6.2.2 — the *first* token of a kind, created.
    ///
    /// The other half of [`Self::confirm_issued`], and not the same rule: this one is
    /// a write, it happens at most once per kind per conversation, and it is skipped
    /// whenever the endpoint is already confirming one.
    ///
    /// §6.2.1 lets the other agent add its own conversation token beside the peer's;
    /// this engine exercises that option only when the peer brought none, so that a
    /// deployment keying its state on the peer's identifier is not handed a second one
    /// to store. A deployment that wants its own identifier beside the peer's mints it
    /// in its handler, and its [`TokenAuthority`] recognises it as own.
    ///
    /// A token *this* endpoint issued does not count as the peer's own — when the
    /// conversation that minted it is gone, leaving the peer holding it would promise
    /// something this endpoint cannot deliver, and minting a replacement is what lets
    /// the peer see that it restarted.
    fn mint_if_needed(&self, request: &Message, reply: &mut Message, cx: &Context) {
        // No let-chains: the workspace's MSRV is 1.85 and they are not stable there.
        for (wanted, kind, is_kind) in [
            (
                self.config.issue_conversation_token,
                TokenKind::Conversation,
                Submessage::is_conversation_token as fn(&Submessage) -> bool,
            ),
            (
                self.config.issue_authentication_token,
                TokenKind::Authentication,
                Submessage::is_authentication_token as fn(&Submessage) -> bool,
            ),
        ] {
            if !wanted {
                continue;
            }
            // Asked again rather than threaded through from `confirm_issued`: it is a
            // read with no side effect, and the two halves are called separately by
            // `close`, which confirms and never mints.
            if self.authority.issued(kind, cx).is_some() {
                continue;
            }
            if self.brought_foreign(request, cx, is_kind) {
                continue;
            }
            if let Some(token) = self.authority.mint(kind, cx) {
                push_token(reply, kind.subformat(), token);
            }
        }
    }

    /// `true` when the request carries a token of that kind which this endpoint did
    /// not issue — the peer's own identifier, which it is entitled to keep.
    fn brought_foreign(
        &self,
        request: &Message,
        cx: &Context,
        kind: fn(&Submessage) -> bool,
    ) -> bool {
        let first = first_submessage_token(request);
        first
            .iter()
            .chain(request.tokens())
            .filter(|token| kind(token))
            .filter_map(Submessage::content_as_str)
            .any(|content| !self.authority.is_own(content, cx))
    }

    /// Refuse to send a reply that breaks clause 5.
    ///
    /// An endpoint producing one is a bug, and a malformed reply is worse than an
    /// error reply: the peer would have to guess what was meant. jsonrpsee's nearest
    /// equivalent yields the string "response could not be serialized"; this replaces
    /// the answer with a protocol-legal error message and hands the findings back so
    /// the runtime can say which rule was broken.
    fn sanitise(&self, request: &Message, reply: Message) -> (Message, Findings) {
        let mut findings = Findings::default();
        let mut fatal = false;
        for violation in reply.validate() {
            let disposition = self.rejection.outbound.of(violation.severity);
            fatal |= disposition == Disposition::Reject;
            findings.push(violation, disposition);
        }

        if !fatal {
            return (reply, findings);
        }

        // The error reply echoes the request's shape where it can, so a control
        // exchange stays a control exchange even when the endpoint failed — by §6.3's
        // boolean alone. `MessageType` stays `Error`: §5.1.1 gives the field one
        // value, §6.3 names the boolean and no other field, and `Error` is the one
        // in-band marker a peer has for a failure (TR/113 §10, ECMA-432 §11).
        let mut replacement = Message::error(
            "the endpoint produced a reply that does not conform to ECMA-430 and it was withheld",
        );
        if request.is_control() {
            replacement.control = Some(true);
        }
        (replacement, findings)
    }
}

impl<E: Endpoint, A: TokenAuthority> Endpoint for Conformity<E, A> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// `check`, then the endpoint, then `finish` — one `await` and no task.
    ///
    /// The findings of either synchronous step are dropped here, because an
    /// [`Endpoint`] has nowhere to put them. A runtime that wants them calls the three
    /// steps itself; that is what they are public for.
    async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        self.check(request, cx)?;
        let reply = self.inner.handle(request, cx).await?;
        Ok(self.finish(request, reply, cx).message)
    }
}

/// §6.3 — "If a NLIP end-point receives a message which has the control field set to
/// true, it must respond with a NLIP Message with control field set to true."
///
/// The boolean is **overwritten**, not filled. The clause is unconditional — unlike
/// §6.2 ¶2 it carries no exception — and from the peer's side "the end-point" is the
/// whole stack, handler included, so a handler that wrote `Some(false)` in answer to a
/// control request would otherwise send a reply the clause forbids. This wrapper
/// already censors on §6.2, where it adds tokens the handler did not write, and on
/// clause 5, where it replaces the reply outright; §6.3 is the weakest of the three.
///
/// `messagetype` is filled and never overwritten. §6.3 names the boolean and no other
/// field, and a handler's own value — `Error`, say — is its answer's classification
/// rather than a claim about control-ness, which is what the replacement error message
/// in [`Conformity::finish`] relies on. Both spellings of the request are answered in
/// kind, so a peer that speaks only `messagetype` and one that speaks only the boolean
/// each see what they sent; neither field is invented where the peer sent neither,
/// because §5.1.1's value and §6.3's boolean are two spellings of one thing and
/// answering in the other one would be a claim about a field the exchange never used.
fn carry_control(request: &Message, reply: &mut Message) {
    if !request.is_control() {
        return;
    }
    reply.control = Some(true);
    if reply.message_type.is_none() {
        reply.message_type = request.message_type.clone();
    }
}

fn is_already_present(reply: &Message, token: &Submessage) -> bool {
    reply.submessages.iter().any(|existing| {
        existing.format == token.format
            && existing.subformat == token.subformat
            && existing.content == token.content
    })
}

fn push_token(reply: &mut Message, subformat: &str, content: String) {
    let already_present = reply.submessages.iter().any(|submessage| {
        submessage.is_token()
            && submessage.subformat.eq_ignore_ascii_case(subformat)
            && submessage.content.as_str() == Some(content.as_str())
    });
    if !already_present {
        reply
            .submessages
            .push(Submessage::token(subformat, content));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenAuthority;
    use nlipsy_protocol::message::Format;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Unique across the whole test binary, so that two authorities sharing a key —
    /// the "same deployment after a restart" case — never mint the same value twice.
    static MINTED: AtomicUsize = AtomicUsize::new(0);

    /// The two answers to "did this endpoint create this token?", in memory.
    ///
    /// A deployment's real authority is a session table *and* a signature, and the
    /// tests that matter are about the gap between them. Here the session is a pair of
    /// `Option<String>` that a fresh authority does not have, and the signature is a
    /// key prefix that survives one — which is exactly the asymmetry a real deployment's
    /// session table and token signature have.
    #[derive(Debug)]
    struct TestAuthority {
        key: &'static str,
        issued: Mutex<Issued>,
    }

    #[derive(Debug, Default)]
    struct Issued {
        conversation: Option<String>,
        authentication: Option<String>,
    }

    impl TestAuthority {
        fn new(key: &'static str) -> TestAuthority {
            TestAuthority {
                key,
                issued: Mutex::new(Issued::default()),
            }
        }

        /// A value this key signed, remembered by nobody — what a restart or an
        /// eviction leaves a peer holding.
        fn sign(&self) -> String {
            format!("{}.{}", self.key, MINTED.fetch_add(1, Ordering::Relaxed))
        }

        fn signed_by_us(&self, token: &str) -> bool {
            token
                .split_once('.')
                .is_some_and(|(key, _)| key == self.key)
        }
    }

    impl TestAuthority {
        fn slot<R>(&self, kind: TokenKind, with: impl FnOnce(&mut Option<String>) -> R) -> R {
            let mut issued = self.issued.lock().unwrap();
            with(match kind {
                TokenKind::Conversation => &mut issued.conversation,
                TokenKind::Authentication => &mut issued.authentication,
            })
        }
    }

    impl TokenAuthority for TestAuthority {
        fn is_own(&self, token: &str, _cx: &Context) -> bool {
            let issued = self.issued.lock().unwrap();
            issued.conversation.as_deref() == Some(token)
                || issued.authentication.as_deref() == Some(token)
                || self.signed_by_us(token)
        }

        /// A read: the session half, and nothing else.
        fn issued(&self, kind: TokenKind, _cx: &Context) -> Option<String> {
            self.slot(kind, |slot| slot.clone())
        }

        /// A write, and only ever called when `issued` said there was nothing.
        fn mint(&self, kind: TokenKind, _cx: &Context) -> Option<String> {
            let fresh = self.sign();
            self.slot(kind, |slot| {
                *slot = Some(fresh.clone());
                Some(fresh)
            })
        }
    }

    /// An endpoint that answers with a fixed message, for the wrapper's own tests.
    struct Fixed(&'static str);

    impl Endpoint for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }

        async fn handle(&self, _request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
            Ok(Message::text(self.0))
        }
    }

    fn cx() -> Context {
        Context::new().with_peer("peer").with_correlator("test")
    }

    /// The layer under test: no endpoint, because the three steps do not need one.
    fn layer_with(config: ConformityConfig, authority: TestAuthority) -> Conformity<(), TestAuthority> {
        Conformity::new((), authority, config, Rejection::default())
    }

    fn layer(config: ConformityConfig) -> Conformity<(), TestAuthority> {
        layer_with(config, TestAuthority::new("ours"))
    }

    fn finish(
        layer: &Conformity<(), TestAuthority>,
        request: &Message,
        reply: Message,
    ) -> Message {
        layer.finish(request, reply, &cx()).message
    }

    /// The shape the reference SDK's `create_token` emits: the whole message *is* a
    /// token, so `Message::tokens` cannot yield it and only `Message::is_token` sees it.
    fn token_message(subformat: &str, content: &str) -> Message {
        Message::new(Format::Token, subformat, content.to_string())
    }

    #[test]
    fn peer_tokens_are_echoed_verbatim() {
        let request = Message::text("hi")
            .with_submessage(Submessage::token("conversation", "abc"))
            .with_submessage(Submessage::token("authentication", "secret"));

        let reply = finish(
            &layer(ConformityConfig::default()),
            &request,
            Message::text("ok"),
        );

        let tokens: Vec<_> = reply.tokens().collect();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].subformat, "conversation");
        assert_eq!(tokens[0].content_as_str(), Some("abc"));
        assert_eq!(tokens[1].subformat, "authentication");
    }

    #[test]
    fn token_labels_and_casing_survive_the_echo() {
        // "the exact same format, subformat and content" — including the peer's own
        // capitalisation of a subformat, since only this endpoint's spelling is ours
        // to normalise.
        let request = Message::text("hi")
            .with_submessage(Submessage::token("Conversation_9.2.3.5", "abc").with_label("seq-1"));

        let reply = finish(
            &layer(ConformityConfig::default()),
            &request,
            Message::text("ok"),
        );
        let echoed = reply.tokens().next().unwrap();
        assert_eq!(echoed.subformat, "Conversation_9.2.3.5");
        assert_eq!(echoed.label.as_deref(), Some("seq-1"));
    }

    #[test]
    fn own_tokens_are_not_echoed_back_at_the_peer() {
        let issued = layer(ConformityConfig::default().with_conversation_token(true));

        // First exchange: the endpoint mints a conversation token.
        let first = finish(&issued, &Message::text("hi"), Message::text("ok"));
        let minted = first.tokens().next().unwrap().content_as_str().unwrap().to_string();

        // Second exchange: the peer returns it. It is the endpoint's own token, so
        // §6.2's exception applies — but it still travels back once, as the
        // conversation's identifier, not as an echo.
        let request =
            Message::text("again").with_submessage(Submessage::token("conversation", &minted));
        let second = finish(&issued, &request, Message::text("ok"));
        let returned: Vec<_> = second.tokens().collect();

        assert_eq!(returned.len(), 1, "the token should appear exactly once");
        assert_eq!(returned[0].content_as_str(), Some(minted.as_str()));
    }

    #[test]
    fn a_token_this_endpoint_issued_is_not_echoed_once_its_session_is_gone() {
        // The case the signature exists for. The authority below has the same key and
        // has never seen this token — which is what a restart leaves behind, and what
        // eviction leaves behind sooner. With the session half alone the answer would
        // be "not ours": the peer's own dead token would go straight back to it,
        // unaltered, and the peer would go on believing the conversation was intact.
        let before = TestAuthority::new("ours");
        let dead = before.mint(TokenKind::Conversation, &cx()).unwrap();

        let issued = layer_with(
            ConformityConfig::default().with_conversation_token(true),
            TestAuthority::new("ours"),
        );
        let request =
            Message::text("again").with_submessage(Submessage::token("conversation", &dead));
        let reply = finish(&issued, &request, Message::text("ok"));

        let returned: Vec<_> = reply.tokens().collect();
        assert_eq!(returned.len(), 1, "still exactly one conversation token");
        assert_ne!(
            returned[0].content_as_str(),
            Some(dead.as_str()),
            "the token whose conversation is gone must not come back"
        );
    }

    #[test]
    fn the_replacement_carries_the_conversation_that_follows() {
        // The other half: the peer adopts what it was handed and the next exchange is
        // not replaced again. Without this the fix would restart the conversation
        // forever.
        let gone = TestAuthority::new("ours");
        let dead = gone.mint(TokenKind::Conversation, &cx()).unwrap();

        let issued = layer_with(
            ConformityConfig::default().with_conversation_token(true),
            TestAuthority::new("ours"),
        );

        let first = finish(
            &issued,
            &Message::text("again").with_submessage(Submessage::token("conversation", &dead)),
            Message::text("ok"),
        );
        let replacement = first.tokens().next().unwrap().content_as_str().unwrap().to_string();

        let second = finish(
            &issued,
            &Message::text("and again")
                .with_submessage(Submessage::token("conversation", &replacement)),
            Message::text("ok"),
        );
        assert_eq!(
            second.tokens().next().unwrap().content_as_str(),
            Some(replacement.as_str()),
            "the replacement should now be the conversation's identifier"
        );
    }

    #[test]
    fn a_token_signed_somewhere_else_counts_as_the_peers_own() {
        // The signature is a claim about authorship, not a rubber stamp: a token from
        // another deployment is the peer's identifier, comes back unchanged, and —
        // because the peer brought one — stops this endpoint adding a second.
        let stranger = TestAuthority::new("theirs");
        let theirs = stranger.mint(TokenKind::Conversation, &cx()).unwrap();

        let request =
            Message::text("hi").with_submessage(Submessage::token("conversation", &theirs));
        let reply = finish(
            &layer(ConformityConfig::default().with_conversation_token(true)),
            &request,
            Message::text("ok"),
        );

        let returned: Vec<_> = reply.tokens().collect();
        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].content_as_str(), Some(theirs.as_str()));
    }

    #[test]
    fn authentication_tokens_follow_the_same_rule() {
        let gone = TestAuthority::new("ours");
        let dead = gone.mint(TokenKind::Authentication, &cx()).unwrap();

        let issued = layer_with(
            ConformityConfig::default().with_authentication_token(true),
            TestAuthority::new("ours"),
        );
        let request =
            Message::text("hi").with_submessage(Submessage::token("authentication", &dead));
        let reply = finish(&issued, &request, Message::text("ok"));

        let returned: Vec<_> = reply.tokens().collect();
        assert_eq!(returned.len(), 1);
        assert_ne!(returned[0].content_as_str(), Some(dead.as_str()));
    }

    #[test]
    fn a_refused_request_gets_the_peers_tokens_back_but_not_our_dead_one() {
        // A runtime's failure path builds a reply with no handler in sight, and owes
        // §6.2 the same echo. `echo_into` is the one loop both paths use.
        let authority = TestAuthority::new("ours");
        let dead = authority.sign();
        let request = Message::text("hi")
            .with_submessage(Submessage::token("conversation", &dead))
            .with_submessage(Submessage::token("conversation", "a-peers-own"));

        let mut reply = Message::error("no");
        layer_with(ConformityConfig::default(), authority).echo_into(&request, &mut reply, &cx());

        let returned: Vec<_> = reply.tokens().collect();
        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].content_as_str(), Some("a-peers-own"));
    }

    #[test]
    fn peer_conversation_token_is_echoed_on_every_exchange() {
        let plain = layer(ConformityConfig::default());
        let request =
            Message::text("hi").with_submessage(Submessage::token("conversation", "peer-owned"));

        for _ in 0..3 {
            let reply = finish(&plain, &request, Message::text("ok"));
            assert_eq!(reply.tokens().count(), 1);
        }
    }

    #[test]
    fn an_own_conversation_token_is_still_confirmed_beside_a_foreign_one() {
        // The case that makes `issued` and `mint` two methods. §6.2.1 lets the peer
        // keep an identifier of its own beside this endpoint's — "the other agent can
        // optionally add another submessage … if it wants to maintain its own
        // conversation identifier" — and the endpoint still owes its own back, because
        // that is how the peer sees the conversation is still the same one. §6.2's
        // exception withholds it from the *echo*, and the guard against minting a
        // second one is on, so issuance is the only path left for it.
        let issued = layer(ConformityConfig::default().with_conversation_token(true));

        let first = finish(&issued, &Message::text("hi"), Message::text("ok"));
        let ours = first
            .tokens()
            .next()
            .unwrap()
            .content_as_str()
            .unwrap()
            .to_string();

        let request = Message::text("again")
            .with_submessage(Submessage::token("conversation", &ours))
            .with_submessage(Submessage::token("conversation_theirs", "peer-owned"));
        let second = finish(&issued, &request, Message::text("ok"));

        let returned: Vec<&str> = second
            .tokens()
            .map(|token| token.content_as_str().unwrap())
            .collect();
        assert!(
            returned.contains(&"peer-owned"),
            "the peer's own comes straight back (§6.2): {returned:?}"
        );
        assert_eq!(
            returned.iter().filter(|token| **token == ours).count(),
            1,
            "and this endpoint's own is confirmed exactly once (§6.2.1): {returned:?}"
        );
        assert_eq!(returned.len(), 2, "and nothing else is invented: {returned:?}");
    }

    #[test]
    fn server_does_not_mint_a_token_when_the_peer_brought_its_own() {
        let request =
            Message::text("hi").with_submessage(Submessage::token("conversation", "peer-owned"));

        let reply = finish(
            &layer(ConformityConfig::default().with_conversation_token(true)),
            &request,
            Message::text("ok"),
        );

        assert_eq!(reply.tokens().count(), 1);
        assert_eq!(
            reply.tokens().next().unwrap().content_as_str(),
            Some("peer-owned")
        );
    }

    #[test]
    fn anonymous_mode_issues_an_authentication_token_once() {
        let anonymous = layer(ConformityConfig::default().with_authentication_token(true));

        let first = finish(&anonymous, &Message::text("hi"), Message::text("ok"));
        let issued = first.tokens().next().unwrap().clone();
        assert_eq!(issued.subformat, "authentication");

        // The peer returns it; the endpoint recognises it as its own and keeps
        // answering with the same value rather than minting a second one.
        let request = Message::text("again").with_submessage(Submessage::token(
            "authentication",
            issued.content_as_str().unwrap(),
        ));
        let second = finish(&anonymous, &request, Message::text("ok"));
        assert_eq!(second.tokens().count(), 1);
        assert_eq!(second.tokens().next().unwrap().content, issued.content);
    }

    #[test]
    fn control_requests_get_control_replies() {
        let plain = layer(ConformityConfig::default());

        let by_type = Message::text("?")
            .message_type("Control")
            .with_submessage(Submessage::token("conversation", "abc"));
        let reply = finish(&plain, &by_type, Message::text("policies"));
        assert!(reply.is_control());
        assert_eq!(reply.message_type.as_deref(), Some("Control"));

        let mut by_flag = Message::new(Format::Text, "english", "?".to_string())
            .with_submessage(Submessage::token("conversation", "abc"));
        by_flag.control = Some(true);
        let reply = finish(&plain, &by_flag, Message::text("policies"));
        assert_eq!(reply.control, Some(true));
        assert!(
            reply.message_type.is_none(),
            "no messagetype was sent, none is invented"
        );

        // §6.3 carries no exception, and "the end-point" is the whole stack from the
        // peer's side: a handler that answers a control request with an explicit
        // `false` is overruled rather than composed around.
        let mut refuses = Message::text("no");
        refuses.control = Some(false);
        let reply = finish(&plain, &by_type, refuses);
        assert_eq!(reply.control, Some(true));
        // And only the boolean: `messagetype` is the handler's classification of its
        // own answer, which §6.3 says nothing about.
        assert_eq!(reply.message_type.as_deref(), Some("Control"));

        let mut classified = Message::error("no");
        classified.control = Some(false);
        let reply = finish(&plain, &by_type, classified);
        assert_eq!(reply.control, Some(true));
        assert_eq!(reply.message_type.as_deref(), Some("Error"));
    }

    #[test]
    fn data_exchanges_are_not_marked_as_control() {
        let reply = finish(
            &layer(ConformityConfig::default()),
            &Message::text("hi"),
            Message::text("ok"),
        );
        assert!(!reply.is_control());
    }

    #[test]
    fn a_malformed_endpoint_reply_is_replaced_by_a_protocol_legal_error() {
        let broken = Message::new(
            Format::Text,
            "english",
            nlipsy_protocol::content::Content::Bool(true),
        );

        let finished =
            layer(ConformityConfig::default()).finish(&Message::text("hi"), broken, &cx());

        assert_eq!(finished.message.message_type.as_deref(), Some("Error"));
        assert!(finished.message.content.is_string());
        // The runtime is told why, rather than the finding being swallowed here.
        assert_eq!(finished.findings.rejected().count(), 1);
    }

    #[test]
    fn a_control_exchange_stays_control_even_when_the_endpoint_fails() {
        let request = Message::text("?").message_type("Control");
        let broken = Message::new(
            Format::Text,
            "english",
            nlipsy_protocol::content::Content::Bool(true),
        );

        let reply = finish(&layer(ConformityConfig::default()), &request, broken);

        // §6.3 is satisfied by the boolean, which is the only field it names, and the
        // one in-band marker of a failure survives: §5.1.1 gives `MessageType` one
        // value, so a reply cannot spell both `Control` and `Error` there.
        assert!(reply.is_control());
        assert_eq!(reply.control, Some(true));
        assert_eq!(reply.message_type.as_deref(), Some("Error"));
    }

    #[test]
    fn close_restores_the_echo_and_the_carry_over_a_reply_finish_never_saw() {
        // What a binding runs over a reply a layer produced on its own: the peer's
        // token is back, the endpoint's issued one is confirmed, §6.3 is answered —
        // and nothing was minted, because minting is a write and this path does not
        // commit one.
        let ours = TestAuthority::new("ours");
        let issued = ours.mint(TokenKind::Conversation, &cx()).unwrap();
        let layer = layer_with(
            ConformityConfig::default().with_conversation_token(true),
            ours,
        );

        let request = Message::text("?")
            .message_type("Control")
            .with_submessage(Submessage::token("authorization", "the-peer's-own"));
        let mut cached = Message::text("from the cache");
        layer.close(&request, &mut cached, &cx());

        let returned: Vec<_> = cached
            .tokens()
            .map(|token| token.content_as_str().unwrap().to_string())
            .collect();
        assert_eq!(returned, ["the-peer's-own".to_string(), issued]);
        assert_eq!(cached.control, Some(true));

        // Idempotent: a binding that runs it after `finish` changes nothing.
        let before = cached.clone();
        layer.close(&request, &mut cached, &cx());
        assert_eq!(cached.submessages, before.submessages);
        assert_eq!(cached.control, before.control);
    }

    #[test]
    fn close_never_mints_where_finish_would() {
        let layer = layer(ConformityConfig::default().with_conversation_token(true));

        let mut cached = Message::text("from the cache");
        layer.close(&Message::text("hi"), &mut cached, &cx());
        assert_eq!(
            cached.tokens().count(),
            0,
            "a token minted on a path that does not commit one is a token the peer \
             holds and this endpoint has forgotten"
        );

        // The same exchange through `finish` does mint one.
        let finished = finish(&layer, &Message::text("hi"), Message::text("ok"));
        assert_eq!(finished.tokens().count(), 1);
    }

    #[test]
    fn too_many_tokens_are_refused_rather_than_truncated() {
        let plain = layer(ConformityConfig::default());
        let mut request = Message::text("hi");
        for index in 0..=DEFAULT_MAX_TOKENS_PER_MESSAGE {
            request =
                request.with_submessage(Submessage::token("conversation", format!("t{index}")));
        }

        assert_eq!(
            plain.check(&request, &cx()),
            Err(Rejected::TooManyTokens {
                count: DEFAULT_MAX_TOKENS_PER_MESSAGE + 1,
                limit: DEFAULT_MAX_TOKENS_PER_MESSAGE,
            })
        );
    }

    #[test]
    fn a_token_in_the_first_submessage_is_echoed_into_the_reply() {
        let request = token_message("Conversation_9.2.3.5", "abc");

        let reply = finish(
            &layer(ConformityConfig::default()),
            &request,
            Message::text("ok"),
        );

        // §6.2 ¶2 asks for "the same submessage with the exact same format, subformat
        // and content" — a submessage, not a position. The reply's own first
        // submessage is the endpoint's answer, so the echo goes into `submessages`.
        let echoed: Vec<_> = reply.tokens().collect();
        assert_eq!(echoed.len(), 1);
        assert_eq!(echoed[0].format, Format::Token);
        assert_eq!(echoed[0].subformat, "Conversation_9.2.3.5");
        assert_eq!(echoed[0].content_as_str(), Some("abc"));
        assert_eq!(reply.content_as_str(), Some("ok"), "the answer is untouched");
    }

    #[test]
    fn a_first_submessage_token_of_ours_is_not_echoed_back_at_the_peer() {
        // §6.2's exception is about authorship and says nothing about where the
        // submessage sits, so the first submessage is judged by the same rule.
        let authority = TestAuthority::new("ours");
        let ours = authority.sign();

        let reply = finish(
            &layer_with(ConformityConfig::default(), authority),
            &token_message("conversation", &ours),
            Message::text("ok"),
        );

        assert_eq!(reply.tokens().count(), 0);
    }

    #[test]
    fn a_first_submessage_conversation_token_stops_a_second_being_minted() {
        // Two conversation tokens in one exchange leave each side naming a different
        // conversation, which is what `brought_foreign` exists to prevent — and it has
        // to see the first submessage to do it.
        let reply = finish(
            &layer(ConformityConfig::default().with_conversation_token(true)),
            &token_message("conversation", "peer-owned"),
            Message::text("ok"),
        );

        let returned: Vec<_> = reply.tokens().collect();
        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].content_as_str(), Some("peer-owned"));
    }

    #[test]
    fn a_refused_request_that_is_itself_a_token_still_gets_it_back() {
        // A runtime's failure path, where there is no handler to ask.
        let mut reply = Message::error("no");
        layer(ConformityConfig::default()).echo_into(
            &token_message("conversation", "a-peers-own"),
            &mut reply,
            &cx(),
        );

        let returned: Vec<_> = reply.tokens().collect();
        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].content_as_str(), Some("a-peers-own"));
    }

    #[test]
    fn the_token_ceiling_counts_the_first_submessage() {
        // Off by one otherwise: the guard exists so that nothing is silently dropped,
        // and a token it cannot see is a token it cannot promise to return.
        let plain = layer(ConformityConfig::default());
        let mut request = token_message("conversation", "t0");
        for index in 1..=DEFAULT_MAX_TOKENS_PER_MESSAGE {
            request =
                request.with_submessage(Submessage::token("conversation", format!("t{index}")));
        }

        assert_eq!(
            plain.check(&request, &cx()),
            Err(Rejected::TooManyTokens {
                count: DEFAULT_MAX_TOKENS_PER_MESSAGE + 1,
                limit: DEFAULT_MAX_TOKENS_PER_MESSAGE,
            })
        );
    }

    #[test]
    fn an_endpoint_cannot_forge_one_of_our_own_tokens() {
        // The authority decides ownership, not the reply, so an endpoint that happens
        // to emit a token value this deployment never issued cannot stop it being
        // echoed — it is deduplicated instead.
        let request = Message::text("hi").with_submessage(Submessage::token("conversation", "abc"));
        let reply_with_same =
            Message::text("ok").with_submessage(Submessage::token("conversation", "abc"));

        let reply = finish(&layer(ConformityConfig::default()), &request, reply_with_same);
        assert_eq!(
            reply.tokens().count(),
            1,
            "identical token must not be duplicated"
        );
    }

    // ---------------------------------------------------------------------
    // The policy values, which the seed wrote in code rather than in data
    // ---------------------------------------------------------------------

    /// A message clause 5 calls off-spec but interpretable: §5.3 says a token's
    /// content is "an opaque token" and the model reports a non-string one as a
    /// `Warning` rather than an `Error`.
    fn warns() -> Message {
        Message::text("hi").with_submessage(Submessage::new(
            Format::Token,
            "conversation",
            nlipsy_protocol::content::Content::Number(7.into()),
        ))
    }

    #[test]
    fn a_warning_comes_back_as_a_finding_rather_than_a_rejection() {
        let findings = layer(ConformityConfig::default())
            .check(&warns(), &cx())
            .expect("a warning does not refuse the request by default");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings.to_log().count(), 1);
    }

    #[test]
    fn a_stricter_inbound_policy_refuses_what_the_default_logs() {
        // The gateway reading: strangers get the strictest reading of clause 5.
        let strict = Conformity::new(
            (),
            TestAuthority::new("ours"),
            ConformityConfig::default(),
            Rejection::new(
                Policy::new(Disposition::Reject, Disposition::Reject),
                Policy::default(),
            ),
        );
        let refused = strict.check(&warns(), &cx()).unwrap_err();
        assert!(matches!(refused, Rejected::Violations(ref found) if found.len() == 1));
        assert!(refused.to_string().contains("not conforming"));
    }

    #[test]
    fn a_lenient_inbound_policy_accepts_what_the_default_refuses() {
        // The bridge reading: the standard contradicts itself in places, and a strict
        // endpoint would refuse messages the reference SDK emits.
        let lenient = Conformity::new(
            (),
            TestAuthority::new("ours"),
            ConformityConfig::default(),
            Rejection::new(
                Policy::new(Disposition::Ignore, Disposition::Ignore),
                Policy::default(),
            ),
        );
        let broken = Message::new(
            Format::Text,
            "english",
            nlipsy_protocol::content::Content::Bool(true),
        );
        let findings = lenient.check(&broken, &cx()).expect("nothing is refused");
        assert!(findings.to_log().next().is_none(), "ignored, not logged");
    }

    #[test]
    fn the_outbound_policy_is_separate_from_the_inbound_one() {
        // "An endpoint that tolerates a peer's warnings should not therefore tolerate
        // its own." Inbound ignores everything; outbound still refuses a broken reply.
        let mixed = Conformity::new(
            (),
            TestAuthority::new("ours"),
            ConformityConfig::default(),
            Rejection::new(
                Policy::new(Disposition::Ignore, Disposition::Ignore),
                Policy::default(),
            ),
        );
        let broken = Message::new(
            Format::Text,
            "english",
            nlipsy_protocol::content::Content::Bool(true),
        );
        let finished = mixed.finish(&Message::text("hi"), broken, &cx());
        assert_eq!(finished.message.message_type.as_deref(), Some("Error"));
    }

    #[test]
    fn the_ceiling_is_configuration_rather_than_a_constant() {
        let tight = layer(ConformityConfig::default().with_max_tokens_per_message(1));
        let request = Message::text("hi")
            .with_submessage(Submessage::token("conversation", "a"))
            .with_submessage(Submessage::token("conversation", "b"));
        assert_eq!(
            tight.check(&request, &cx()),
            Err(Rejected::TooManyTokens { count: 2, limit: 1 })
        );
    }

    // ---------------------------------------------------------------------
    // The wrapper, which is the one thing here that needs an executor
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn the_wrapper_is_check_then_handle_then_finish() {
        let wrapped = Conformity::new(
            Fixed("ok"),
            TestAuthority::new("ours"),
            ConformityConfig::default(),
            Rejection::default(),
        );
        let request = Message::text("?")
            .message_type("Control")
            .with_submessage(Submessage::token("conversation", "abc"));

        let reply = wrapped.handle(&request, &cx()).await.unwrap();

        assert_eq!(wrapped.name(), "fixed");
        assert_eq!(reply.content_as_str(), Some("ok"));
        assert_eq!(reply.tokens().next().unwrap().content_as_str(), Some("abc"));
        assert!(reply.is_control());
    }

    #[tokio::test]
    async fn a_refused_request_never_reaches_the_endpoint() {
        // And the refusal is a variant of its own, because a binding has to tell "the
        // peer's message was wrong" from "this endpoint failed" — 4xx from 5xx.
        let wrapped = Conformity::new(
            Fixed("ok"),
            TestAuthority::new("ours"),
            ConformityConfig::default().with_max_tokens_per_message(0),
            Rejection::default(),
        );
        let request =
            Message::text("hi").with_submessage(Submessage::token("conversation", "abc"));

        let error = wrapped.handle(&request, &cx()).await.unwrap_err();
        assert!(matches!(
            error,
            EndpointError::Refused(Rejected::TooManyTokens { .. })
        ));
        assert!(error.public_message().contains("echoes at most 0"));
    }
}
