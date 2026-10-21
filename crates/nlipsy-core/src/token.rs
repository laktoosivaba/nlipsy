//! §6.2's exception, as a trait: "except when the token was initially created by the
//! end-point".
//!
//! ECMA-430 §6.2 ¶2 obliges an endpoint to return every `token` submessage it was
//! sent, "with the exact same format, subformat and content", and then takes one thing
//! back: a token the endpoint itself created need not come back. Deciding that is the
//! only question clause 6 asks about a token's *value*, and it is a question no
//! library can answer — the answer is a property of how the deployment mints them.
//!
//! So it is a trait with two methods and no state. §6.2.1 says the endpoint that mints
//! a conversation token has to keep returning it, and "keeping" is state; the
//! implementor owns that state, which is where interior mutability belongs. The
//! alternative — having the core return a "token effect" for the binding to apply —
//! was considered and rejected: it moves the commit step into every binding, which is
//! precisely the step that is easiest to forget.

use crate::context::Context;
use crate::send::MaybeSend;

/// Which of ECMA-430's two reserved token kinds is being asked for.
///
/// Exhaustive: §5.3 reserves `authentication` and `conversation` and §6.2 says of
/// every other subformat that "the use of these tokens is up to the application" — so
/// a third variant would be a token this crate has no rule about, and an endpoint that
/// wants one mints it in its own handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// §6.2.1 — the identifier that makes a sequence of requests one conversation.
    Conversation,
    /// §6.2.2 — the credential returned unmodified so authentication is seamless.
    Authentication,
}

impl TokenKind {
    /// The subformat this crate writes when it issues a token of this kind.
    ///
    /// §5.3 reserves the two prefixes and §6.2.1 gives the part after `_` a meaning —
    /// the identity of the agent that created the conversation — so the bare prefix is
    /// what an endpoint with nothing to add writes. Recognition is not done by this
    /// string: `Submessage::is_conversation_token` matches the prefix, whatever
    /// follows it.
    pub const fn subformat(self) -> &'static str {
        match self {
            TokenKind::Conversation => "conversation",
            TokenKind::Authentication => "authentication",
        }
    }
}

/// Who created a token, and what this endpoint's own tokens are.
///
/// Two methods, and they answer different questions. [`is_own`](TokenAuthority::is_own)
/// is §6.2's exception and is asked of every token in a request.
/// [`mint`](TokenAuthority::mint) is §6.2.1 and §6.2.2 and is asked at most once per
/// kind per exchange.
///
/// # What an implementation usually is
///
/// A deployment that keeps sessions answers `is_own` as a disjunction: the session
/// remembers what it minted *while it is still here*, and a signature is recognisable
/// whether or not the conversation behind it survived. Both halves are needed, and the
/// second is the one that is easy to leave out: without it, a token this endpoint
/// minted before a restart comes back looking like a stranger's, is echoed, and tells
/// the peer its conversation is intact when the endpoint has no idea whether it is.
/// The session half needs the request's session, which is why `cx` is a parameter.
///
/// A deployment with no sessions implements the signature half alone. A deployment
/// that issues nothing implements `is_own` as `false` and takes the two defaults.
///
/// # Why `issued` and `mint` are two methods and not one
///
/// They answer different questions and are asked under different conditions, and
/// collapsing them loses a token. §6.2.1 obliges the end-point that created a
/// conversation token to keep confirming it — that is how the peer sees the
/// conversation is still the same one — so [`issued`](TokenAuthority::issued) is
/// consulted on *every* reply, whatever else the request carried. Minting is the
/// opposite: it happens once, and only when the peer has brought no identifier of its
/// own. §6.2.1 lets the other agent add its own conversation token beside the peer's;
/// this engine exercises that option only when the peer brought none, so that a
/// deployment keying its state on the peer's identifier is not handed a second one to
/// store. A deployment that wants its own identifier beside the peer's mints it in its
/// handler, and its `TokenAuthority` recognises it as own.
///
/// A single method would have to be both, and the case that breaks it is real: a peer
/// that returns this end-point's conversation token *and* a token of its own. The
/// endpoint's own is withheld by §6.2's exception (it is `is_own`), the guard against
/// minting is on, and a combined method is never called — so the reply would drop the
/// identifier the endpoint is obliged to keep confirming.
///
/// **Both must be cheap and non-blocking.** They run inside the request, on the path
/// between the handler and the wire. A lock held for as long as a `HashMap` lookup is
/// fine; a round trip to a database is not, and a deployment that needs one should
/// mint ahead of time rather than here.
pub trait TokenAuthority: MaybeSend + Sync + 'static {
    /// `true` when this endpoint created `token`, and may therefore decline to echo it.
    ///
    /// The value is opaque: §6.2 calls a token "interpretable only by the end-point
    /// creating it", so a token this returns `false` for comes straight back to the
    /// peer unchanged, whoever wrote it.
    ///
    /// It is asked with the empty string for a token whose content is not a string —
    /// a number, an object — and the answer for that is `false`, because an
    /// implementation that claimed `""` as its own would withhold every such token.
    /// Nothing is lost by declining it:
    /// [`Conformity::echo_into`](crate::Conformity::echo_into) pushes the submessage
    /// itself rather than its content, so the token still comes back whole and §6.2 ¶2's
    /// "the exact same content" is honoured either way, while the only other reader of
    /// this answer — the guard that stops §6.2.1 minting beside a token the peer
    /// brought — skips a non-string token instead of counting it, so such a token never
    /// suppresses a mint.
    fn is_own(&self, token: &str, cx: &Context) -> bool;

    /// The token of `kind` this endpoint has **already** issued for this conversation.
    ///
    /// A read with no side effect: it creates nothing, and returning `None` is how an
    /// implementation says "nothing has been issued for this conversation yet".
    ///
    /// A token this endpoint issued comes back on every reply the binding builds,
    /// refusals included — see [`Conformity::close`](crate::Conformity::close) — which
    /// is how the peer sees the conversation is still the same one. Dropping it would
    /// leave the peer holding an identifier the server no longer confirms, which is
    /// precisely what §6.2.1 asks an endpoint not to do.
    ///
    /// The default is `None`, which is what an endpoint that maintains no conversation
    /// state wants: §6.2 makes session state optional.
    fn issued(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
        None
    }

    /// Create a **new** token of `kind` for this conversation and remember it.
    ///
    /// Called only when [`issued`](TokenAuthority::issued) returned `None` *and* the
    /// request brought no token of that kind this endpoint did not create — so an
    /// implementation does not have to check either, and does not have to be
    /// idempotent. Remembering is its job: the value returned here is what
    /// [`issued`](TokenAuthority::issued) must answer with from the next exchange on.
    ///
    /// Returning `None` declines to issue one, which is always allowed: §6.2.1 and
    /// §6.2.2 are optional.
    fn mint(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
        None
    }
}
