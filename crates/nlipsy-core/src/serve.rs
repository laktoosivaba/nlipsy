//! The sequence a binding drives around clause 6, and what a failure still owes.
//!
//! [`Conformity`] holds the *obligations* — check a request, finish a reply, close over
//! one `finish` never saw. This module holds the **order they run in**, which is the
//! other half of being an end-point and was, until it moved here, written out inside
//! `nlipsy-server`:
//!
//! 1. decode the bytes (or take a message already decoded);
//! 2. [`Conformity::check`], *outside* the layer stack, so that a message the end-point
//!    may not answer never reaches a layer that would admit a conversation for it;
//! 3. the deployment's [`Middleware`](crate::Middleware) stack, with
//!    [`Conformity::finish`] at the **bottom** of it;
//! 4. [`Conformity::close`] over whatever came back, **unconditionally**;
//! 5. on any failure, the rules of [`owed`] and [`error_reply`] instead.
//!
//! Steps 3 and 4 are one property and are the reason this is a composer rather than a
//! comment in a README. `finish` is at the bottom because §6.2.1's token is *minted*
//! there, and a deployment that persists what it minted has to run after that — its
//! layer's after-half must see the reply that actually goes on the wire. `close` runs
//! after the stack because a layer may answer `Ok` without calling
//! [`Next::run`](crate::Next::run) at all, and because a layer's after-half holds the
//! finished reply by value and can strip a token out of it. Both are ordinary things to
//! write; neither may take from the peer what clause 6 owes it.
//!
//! # Why this is in the core and not in each binding
//!
//! Nothing in the list above is HTTP. It is one `await` over an [`Endpoint`], three
//! synchronous steps and a `match`, and a second binding — ECMA-432's frames, ECMA-433's
//! AMQP properties, a `wasi:http` handler in the component reaching this crate as wasm —
//! would otherwise re-derive the order and get one of the two properties above wrong.
//! What a binding keeps is everything this module has no opinion about: reading the
//! bytes, choosing a status code or a frame header, logging, and the clock.
//!
//! [`Conformity`]'s own [`Endpoint`] implementation stays what it was — `check`, the
//! endpoint, `finish`, one `await` and no task — for a caller that wants clause 6 around
//! a handler and nothing else. This is what a *binding* drives.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nlipsy_protocol::content::Wire;
use nlipsy_protocol::error::ParseError;
use nlipsy_protocol::message::Message;

use crate::conformity::{Conformity, Findings, Finished, Rejected};
use crate::context::Context;
use crate::endpoint::{Endpoint, EndpointError};
use crate::middleware::{DynMiddleware, Next};
use crate::send::MaybeSend;
use crate::token::TokenAuthority;

/// How one exchange ended.
///
/// The four ways an end-point can finish with a peer's message, which is also the
/// vocabulary a binding maps onto its own failure surface — a status line, a frame, an
/// AMQP property. Every one of them still produces a reply: [`Served::reply`] is a
/// message on all four, because a peer reading only the payload has to learn what
/// happened.
///
/// `#[non_exhaustive]`: a later clause-6 guard adds a way to refuse. Match with a `_`
/// arm, or use [`public_message`](Outcome::public_message), which every variant answers.
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
    /// The end-point answered. The reply is its answer, finished and closed — which
    /// does not mean clause 5 accepted it; see [`Served::withheld`].
    Ok,
    /// The bytes were not an NLIP message at all, so there was never a request.
    Malformed(ParseError),
    /// The message was refused before the end-point saw it: clause 5 under a policy
    /// that rejects, or more tokens than this end-point will echo.
    Refused(Rejected),
    /// The end-point — or a layer above it — could not answer.
    Failed(EndpointError),
}

impl Outcome {
    /// The text the peer may be shown, or `None` when there is nothing to explain.
    ///
    /// `None` for [`Outcome::Ok`] alone: the reply *is* the answer there. Every other
    /// variant yields a sentence describing the peer's own message or this end-point's
    /// failure, with the detail of an internal fault deliberately left out — ECMA-434
    /// §7.3.3, applied by [`EndpointError::public_message`].
    pub fn public_message(&self) -> Option<String> {
        match self {
            Outcome::Ok => None,
            Outcome::Malformed(error) => Some(format!("malformed NLIP message: {error}")),
            // Safe to hand back: `Rejected` describes the peer's own message, which the
            // peer already has.
            Outcome::Refused(rejected) => Some(rejected.to_string()),
            Outcome::Failed(error) => Some(error.public_message()),
        }
    }

    /// The delay a dependency asked for, when one did.
    ///
    /// A [`Duration`], which is a value; `Retry-After` is one binding's spelling of it
    /// and belongs to that binding (`nlipsy-http`).
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Outcome::Failed(error) => error.retry_after(),
            _ => None,
        }
    }
}

/// One exchange, answered.
///
/// `#[non_exhaustive]`: a binding builds one only through [`serve`] and reads the
/// fields, so a fifth fact about an exchange must not be a breaking change.
#[derive(Debug)]
#[non_exhaustive]
pub struct Served {
    /// What goes back to the peer. A message on every outcome, refusals included.
    pub reply: Message,
    /// How the exchange ended.
    pub outcome: Outcome,
    /// `true` when clause 5 rejected the end-point's *own* answer and [`Served::reply`]
    /// is the replacement rather than the answer — [`Finished::withheld`].
    ///
    /// It is not an [`Outcome`] of its own, and deliberately: the layers above saw a
    /// successful exchange, the session commits and a minted token is persisted, all of
    /// which is right, because the peer's *request* was fine. Only the binding's status
    /// line changes, which is why the fact travels beside the outcome rather than in it.
    ///
    /// [`Finished::withheld`]: crate::Finished::withheld
    pub withheld: bool,
    /// What clause 5 said along the way: the request's findings from
    /// [`Conformity::check`] first, then the reply's from [`Conformity::finish`].
    ///
    /// Data rather than log lines, because this crate has no logger and should not
    /// acquire one — which findings are worth a line, and at what level, is the
    /// binding's decision.
    pub findings: Findings,
}

/// What clause 6 still owes the peer on a reply a binding built itself.
///
/// Three cases and not an `Option<&Message>`, because the token ceiling is a third kind
/// of answer: a refusal that deliberately echoes nothing.
///
/// `#[non_exhaustive]`: a later guard may owe a fourth thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Owed {
    /// Nothing, because there was never a message. The bytes did not decode, so there
    /// is no token to echo and no control-ness to read.
    Nothing,
    /// Everything [`Conformity::close`] discharges: §6.2's echo, §6.2.1's confirmation
    /// of what is already issued, §6.3's carry. Every outcome but two.
    Clause6,
    /// §6.3 alone. The request carried more tokens than this end-point will echo, so
    /// echoing them is exactly the work being refused — the engine's one deliberate
    /// departure from §6.2 ¶2, taken under ECMA-434 §7.2.5 and documented on
    /// [`DEFAULT_MAX_TOKENS_PER_MESSAGE`](crate::DEFAULT_MAX_TOKENS_PER_MESSAGE).
    ControlOnly,
}

/// What `outcome` still owes the peer.
///
/// A rule over a value, with no end-point, no context and no executor in it — which is
/// what lets a binding in another language apply the same rule to the same enum.
///
/// [`Outcome::Ok`] owes [`Owed::Clause6`] like most failures do, and that is not an
/// accident of the `match`: the success path runs [`Conformity::close`] over the layer
/// stack's reply for the same reason a refusal does, so the two paths owe the same
/// thing and differ only in who wrote the message.
pub fn owed(outcome: &Outcome) -> Owed {
    match outcome {
        // The one outcome with no request behind it.
        Outcome::Malformed(_) => Owed::Nothing,
        Outcome::Refused(Rejected::TooManyTokens { .. }) => Owed::ControlOnly,
        _ => Owed::Clause6,
    }
}

/// The reply a refusal or a failure goes back as.
///
/// An NLIP `MessageType: Error` message carrying `why`, with what [`owed`] says clause 6
/// still owes applied to it. Synchronous, and public because a binding produces
/// refusals of its own that never reach [`serve`] — a rate limiter that has decoded the
/// request, a frame the transport rejected — and those owe the peer the same three
/// obligations.
///
/// What clause 6 owes here goes through [`Conformity::close`] — the *same* echo loop and
/// the *same* confirmation the success path uses, which is the point of there being one
/// of each. A refusal is a reply, so a token this end-point issued is confirmed on it
/// too: dropping it would leave the peer holding an identifier this end-point no longer
/// confirms after one throttled turn, which is what §6.2.1 asks an end-point not to do.
/// Nothing is *minted* — see [`Conformity::close`] for why a path a deployment does not
/// commit must not create one.
///
/// A request that fails before a layer admitted anything reaches this with a context
/// that carries none, so authorship is settled by signature alone there: a token this
/// end-point signed is not handed back, which is how a peer learns the conversation it
/// named is not here rather than being told it is.
///
/// `messagetype` stays `Error`. §6.3 names the boolean and no other field, §5.1.1 gives
/// `MessageType` one value, and `Error` is the one in-band marker a peer has for a
/// failure — so a refused control exchange goes back as
/// `{"messagetype":"Error","control":true,…}`.
///
/// `request` is `None` exactly when [`Owed::Nothing`] is: there was no message to echo
/// from. Passing one with `Owed::Nothing`, or none with the other two, is not an error
/// and simply owes nothing.
pub fn error_reply<E, A: TokenAuthority>(
    conformity: &Conformity<E, A>,
    request: Option<&Message>,
    why: &str,
    owed: Owed,
    cx: &Context,
) -> Message {
    let mut reply = Message::error(why);
    match (owed, request) {
        (Owed::Clause6, Some(request)) => conformity.close(request, &mut reply, cx),
        // §6.3 by hand, because the echo is the thing being refused. The boolean is
        // what the clause names, and a reply built from `Message::error` has none of
        // its own to overwrite.
        (Owed::ControlOnly, Some(request)) if request.is_control() => reply.control = Some(true),
        _ => {}
    }
    reply
}

/// Answer one exchange, from the bytes a transport read.
///
/// The whole sequence of this module's opening list. `wire` says how to decode, so that
/// ECMA-432 §7.2's text fallback stays the caller's choice rather than a guess made
/// here — the same reason [`decode_reply`](crate::decode_reply) takes one.
///
/// `cx` is `&mut` because the layers write to it: a session admitted, a principal
/// authenticated, a deadline imposed, all of which the end-point and the clause-6 steps
/// then read. The binding fills in what it knows — the peer, the correlator — before
/// calling this.
///
/// The [`Conformity`] arrives inside an [`Arc`] rather than by reference, and the
/// reason is [`Endpoint`]'s `'static` bound: the value this composer puts at the bottom
/// of the layer stack — the one that calls [`Conformity::finish`] after the handler —
/// is an `Endpoint` itself, so it cannot borrow the wrapper. Sharing it costs one atomic
/// increment per exchange, on a path that has already allocated a decoded message, and a
/// binding holds exactly one wrapper for the life of its end-point.
///
/// The `where` clause is the same asymmetry `impl Endpoint for Arc<T>` spells and
/// [`Dispatcher`](crate::Dispatcher) spells as a `cfg`: `Endpoint` requires `Sync`,
/// `Arc<T>: Sync` requires `T: Send`, and on `wasm32` [`MaybeSend`] is deliberately not
/// `Send`. On a threaded target it is free — `E: Endpoint` gives `E: Send` there, so the
/// bound is proved from the ones already written. On `wasm32` it asks the end-point and
/// the authority to be genuinely `Send`, and the contract of the package that compiles
/// there is what makes that satisfiable: its imports are plain functions and its
/// context is a value (`wit/README.md`, *The design*), so the guest's adapters own nothing
/// borrowed from a host and nothing `!Send` reaches this seam.
///
/// It never returns an error. Every failure is a [`Served`] with an [`Outcome`] and a
/// reply the peer can read, because a binding that has to invent a body for a failure
/// is a binding that will invent a different one from the next binding.
///
/// # Panics
///
/// On [`Wire::Cbor`] when the `cbor` feature is off, for the reason given on
/// [`encode_request`](crate::encode_request): a build with no CBOR codec cannot tell a
/// frame it cannot read from a frame that is malformed, and blaming the peer for the
/// missing feature would be the wrong answer.
pub async fn serve<E: Endpoint, A: TokenAuthority>(
    conformity: &Arc<Conformity<E, A>>,
    layers: &[Arc<dyn DynMiddleware>],
    request: &[u8],
    wire: Wire,
    cx: &mut Context,
) -> Served
where
    Arc<Conformity<E, A>>: MaybeSend + Sync,
{
    let decoded = match wire {
        Wire::Json => Message::from_json_bytes(request),
        #[cfg(feature = "cbor")]
        Wire::Cbor => Message::from_cbor_slice(request),
        #[cfg(not(feature = "cbor"))]
        Wire::Cbor => panic!(
            "Wire::Cbor needs the `cbor` feature of nlipsy-core (ECMA-432 §7.1); \
             this build has no CBOR codec"
        ),
    };

    match decoded {
        Ok(request) => serve_message(conformity, layers, &request, cx).await,
        Err(error) => failed(
            conformity,
            None,
            Outcome::Malformed(error),
            cx,
            Findings::default(),
        ),
    }
}

/// Answer one exchange whose end-point has **already finished** its reply.
///
/// The order of this module's opening list, from step 2 on, with step 3 supplied by the
/// caller rather than composed here: `check`, then `answer`, then
/// [`Conformity::close`] over whatever came back, unconditionally, and on any failure
/// the rules of [`owed`] and [`error_reply`] instead. [`serve_message`] *is* this
/// function with the layer stack as its `answer`, which is what keeps the order in one
/// place.
///
/// It exists because a binding can own the layer stack itself: a caller that composed
/// its own layers and called [`Conformity::finish`] at the bottom of them holds a
/// [`Finished`] and not a bare [`Message`]. Handing that to [`serve_message`] would
/// run [`Conformity::finish`] a **second** time over a reply that has already been
/// through one — and §6.2.1's minting happens there, so an authority that has not yet
/// been told what the first mint was would mint again and hand the peer two
/// identifiers. `close` is the idempotent half and is what a reply that is already
/// finished still owes.
///
/// `answer` takes the context because the caller's stack writes to it and the
/// clause-6 steps on either side read it: it is one borrow, lent for the length of the
/// call and handed back. A caller whose answer needs nothing from it — the guest, which
/// holds the host's own context resource — ignores the parameter.
///
/// Neither [`Arc`] nor an [`Endpoint`] appears in the signature, unlike [`serve`]: both
/// are there for `Tail`, the `'static` end-point this composer puts at the bottom of the
/// stack, and there is no stack here. So the `wasm32` `Send` bound `serve` carries is
/// not owed by this path either.
///
/// It never returns an error, for the reason [`serve`] does not.
pub async fn serve_answered<E, A: TokenAuthority>(
    conformity: &Conformity<E, A>,
    request: &Message,
    cx: &mut Context,
    answer: impl AsyncFnOnce(&mut Context) -> Result<Finished, EndpointError>,
) -> Served {
    // Everything that must be settled before the end-point is asked anything: clause 5
    // under the inbound policy, then §6.2's token ceiling. It runs *outside* whatever
    // the caller put in `answer`, so that a message the end-point may not answer never
    // reaches a layer that would admit a conversation for it.
    let findings = match conformity.check(request, cx) {
        Ok(findings) => findings,
        Err(rejected) => {
            return failed(
                conformity,
                Some(request),
                Outcome::Refused(rejected),
                cx,
                Findings::default(),
            );
        }
    };

    let finished = match answer(cx).await {
        Ok(finished) => finished,
        Err(error) => {
            return failed(conformity, Some(request), Outcome::Failed(error), cx, findings);
        }
    };

    // Read before `close` touches the message: `withheld` is about the end-point's own
    // answer and what clause 5 did to it, which is settled by the time this arrives.
    let withheld = finished.withheld();
    let mut reply = finished.message;

    // Clause 6 over whatever came back, whether or not `finish` ever ran. A layer that
    // answers `Ok` without calling `Next::run` — a cache, a canned answer, a
    // maintenance-mode layer — bypasses `finish` entirely, and a layer's after-half
    // holds the finished reply by value and can strip a token out of it. Both are
    // supported things to write, so the three idempotent obligations are re-applied here
    // rather than assumed: on the ordinary path this is three linear scans and no
    // change. `sanitise` is deliberately *not* re-run — under `outbound.on_warning:
    // Reject` a second pass would reject the peer's own echoed `token-content` warning
    // and replace a good reply.
    conformity.close(request, &mut reply, cx);

    let mut findings = findings;
    findings.extend(finished.findings);

    Served {
        reply,
        outcome: Outcome::Ok,
        withheld,
        findings,
    }
}

/// Answer one exchange whose message is already decoded.
///
/// The same sequence as [`serve`] without step 1, for a binding whose frame carried a
/// message rather than bytes — and for one that wants the request for its own log line
/// or audit trail, which it cannot have if this crate decoded it.
///
/// It is [`serve_answered`] with the deployment's layer stack as its answer, and `Tail`
/// — the end-point with [`Conformity::finish`] under it — at the bottom of that stack.
/// `Tail` is what makes `finish` the bottom rather than something this function does
/// afterwards; see its own documentation for why that is where it has to be.
pub async fn serve_message<E: Endpoint, A: TokenAuthority>(
    conformity: &Arc<Conformity<E, A>>,
    layers: &[Arc<dyn DynMiddleware>],
    request: &Message,
    cx: &mut Context,
) -> Served
where
    Arc<Conformity<E, A>>: MaybeSend + Sync,
{
    let tail = Tail::new(conformity);
    serve_answered(conformity, request, cx, async |cx: &mut Context| {
        let reply = Next::new(layers, &tail).run(request, cx).await?;
        // `Finished` rather than the message alone, because that is what the step below
        // `serve_answered` produces on every other path: the findings are `finish`'s,
        // and `Finished::withheld` derives the flag from them, so the two halves of
        // this crate's own answer and a host's cannot disagree about what withheld
        // means. A layer that short-circuited never reached `Tail`, and the empty
        // findings it leaves behind are the honest answer.
        Ok(Finished {
            message: reply,
            findings: tail.taken(),
        })
    })
    .await
}

/// Everything a failure produces: the error reply, and the outcome beside it.
fn failed<E, A: TokenAuthority>(
    conformity: &Conformity<E, A>,
    request: Option<&Message>,
    outcome: Outcome,
    cx: &Context,
    findings: Findings,
) -> Served {
    // `Outcome::Ok` is the one outcome with nothing to explain, and it does not reach
    // here: every caller below builds a failure.
    let why = outcome.public_message().unwrap_or_default();
    let reply = error_reply(conformity, request, &why, owed(&outcome), cx);
    Served {
        reply,
        outcome,
        // Nothing was withheld: the end-point's own answer is what `withheld` is about,
        // and on these paths there is none.
        withheld: false,
        findings,
    }
}

/// The bottom of the layer stack: the end-point, and clause 6 closing over its answer.
///
/// `finish` is *here*, underneath every layer, rather than in [`serve_message`] above
/// them, and the reason is §6.2.1. Issuing a conversation token happens inside `finish`,
/// through [`TokenAuthority::mint`], and a deployment that keeps what it minted has to
/// persist it *after* that — `sessions.commit` in the reference server. A layer whose
/// "after" half ran before `finish` could only commit a session that had not been given
/// its token yet. Putting the obligations at the bottom is also jsonrpsee's shape: its
/// innermost `RpcService` produces the final response, and every layer's after-half sees
/// the response that actually goes on the wire.
///
/// It *shares* the [`Conformity`] rather than borrowing it, and that is what the `Arc`
/// in [`serve`]'s signature is for: [`Endpoint`] is `'static`, because a stored
/// end-point is — a `Box<dyn DynEndpoint>` chosen from configuration, a router's state
/// — and a type at the bottom of the stack that borrowed the wrapper could not be one.
/// The cost is one atomic increment per exchange, on a path that has already allocated
/// a decoded message; `nlipsy-server` shared its own `Server` handle the same way
/// before this moved here.
struct Tail<E, A> {
    conformity: Arc<Conformity<E, A>>,
    /// What [`Conformity::finish`] said about the end-point's own reply: written once,
    /// at the bottom of the stack, and taken after the stack has unwound.
    ///
    /// The findings alone, and not a flag beside them, because
    /// [`Finished::withheld`](crate::Finished::withheld) is derived from exactly these
    /// — so there is one answer to "was it withheld" on this path and on a host's.
    ///
    /// A [`Mutex`] rather than a cell because [`Endpoint`] is `Sync` and `handle` takes
    /// `&self`: the value is shared with whatever the layers do with it. It is never
    /// contended — one task writes it once — and it is never held across an `await`.
    findings: Mutex<Findings>,
}

impl<E, A> Tail<E, A> {
    fn new(conformity: &Arc<Conformity<E, A>>) -> Tail<E, A> {
        Tail {
            conformity: Arc::clone(conformity),
            findings: Mutex::new(Findings::default()),
        }
    }

    /// What the bottom of the stack recorded, taken out from behind the lock.
    ///
    /// By reference rather than by value because [`serve_message`] lends the `Tail` to
    /// the stack and reads it back inside the same borrow. A poisoned lock means the
    /// handler panicked while this was held, which it cannot: nothing between the two
    /// `lock` calls can unwind. The empty default is the honest answer for a `Tail` that
    /// never ran — which is what a short-circuiting layer produces.
    fn taken(&self) -> Findings {
        match self.findings.lock() {
            Ok(mut findings) => std::mem::take(&mut *findings),
            Err(_) => Findings::default(),
        }
    }
}

impl<E: Endpoint, A: TokenAuthority> Endpoint for Tail<E, A>
where
    Arc<Conformity<E, A>>: MaybeSend + Sync,
{
    fn name(&self) -> &str {
        Endpoint::name(self.conformity.inner())
    }

    async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        let conformity = &self.conformity;
        let reply = conformity.inner().handle(request, cx).await?;
        let finished = conformity.finish(request, reply, cx);
        if let Ok(mut findings) = self.findings.lock() {
            *findings = finished.findings;
        }
        Ok(finished.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use nlipsy_protocol::content::Content;
    use nlipsy_protocol::message::{Format, Submessage};

    use crate::conformity::{ConformityConfig, Rejection};
    use crate::middleware::Middleware;
    use crate::token::TokenKind;

    // ---------------------------------------------------------------------------
    // The pieces. All local and all small: this crate's `testing` feature is the
    // client half's scripted transport, and an end-point that answers from a value
    // needs nothing it does not already have.
    // ---------------------------------------------------------------------------

    /// Answers with what it was sent, so a test can tell the end-point's own answer
    /// from everything clause 6 added around it.
    struct Echo;

    impl Endpoint for Echo {
        fn name(&self) -> &str {
            "echo"
        }

        async fn handle(&self, request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
            Ok(Message::text(request.content_as_str().unwrap_or("")))
        }
    }

    /// Fails the same way every time.
    struct Failing(EndpointError);

    impl Endpoint for Failing {
        fn name(&self) -> &str {
            "failing"
        }

        async fn handle(
            &self,
            _request: &Message,
            _cx: &Context,
        ) -> Result<Message, EndpointError> {
            Err(self.0.clone())
        }
    }

    /// Answers with a message that breaks clause 5 — a `text` part whose content is not
    /// a string (§5.1.4) — so the outbound policy has something to withhold.
    struct NonConforming;

    impl Endpoint for NonConforming {
        fn name(&self) -> &str {
            "non-conforming"
        }

        async fn handle(
            &self,
            _request: &Message,
            _cx: &Context,
        ) -> Result<Message, EndpointError> {
            Ok(Message::new(Format::Text, "english", Content::Bool(true)))
        }
    }

    /// Answers with the request itself, submessages included, so that the reply meets
    /// clause 5 carrying whatever the peer sent — which is how the *reply's* half of
    /// the findings gets something to report.
    struct Mirror;

    impl Endpoint for Mirror {
        fn name(&self) -> &str {
            "mirror"
        }

        async fn handle(&self, request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
            Ok(request.clone())
        }
    }

    /// §6.2's exception, in memory: this end-point's tokens are the ones it minted.
    #[derive(Default)]
    struct Memory {
        conversation: Mutex<Option<String>>,
    }

    impl Memory {
        /// An end-point already in a conversation it named — what a peer's second and
        /// every later request meets.
        fn holding(token: &str) -> Memory {
            Memory {
                conversation: Mutex::new(Some(token.to_string())),
            }
        }
    }

    impl TokenAuthority for Memory {
        fn is_own(&self, token: &str, _cx: &Context) -> bool {
            self.conversation.lock().unwrap().as_deref() == Some(token)
        }

        fn issued(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
            self.conversation.lock().unwrap().clone()
        }

        fn mint(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
            let minted = "minted-1".to_string();
            *self.conversation.lock().unwrap() = Some(minted.clone());
            Some(minted)
        }
    }

    /// Mints a fresh token every time and remembers nothing.
    ///
    /// The shape `serve_answered` exists for: a deployment that persists what it minted
    /// in a layer *above* `finish` has not told its authority anything by the time
    /// `finish` returns, so `issued` still answers `None`. Under that authority a second
    /// `finish` over an already-finished reply mints a second identifier, which is the
    /// hazard the composer variant removes.
    #[derive(Default)]
    struct Minting {
        mints: AtomicUsize,
    }

    impl Minting {
        /// How many tokens this authority has created.
        fn minted(&self) -> usize {
            self.mints.load(Ordering::Relaxed)
        }
    }

    impl TokenAuthority for Minting {
        fn is_own(&self, _token: &str, _cx: &Context) -> bool {
            false
        }

        fn issued(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
            None
        }

        fn mint(&self, _kind: TokenKind, _cx: &Context) -> Option<String> {
            let nth = self.mints.fetch_add(1, Ordering::Relaxed) + 1;
            Some(format!("minted-{nth}"))
        }
    }

    /// A layer that answers on its own without calling `next` — a cache, a canned
    /// answer, a maintenance-mode page. It knows nothing about clause 6, which is the
    /// point: it is an ordinary thing to write and it never reaches `finish`.
    struct Cached;

    impl Middleware for Cached {
        async fn call(
            &self,
            _request: &Message,
            _cx: &mut Context,
            _next: Next<'_>,
        ) -> Result<Message, EndpointError> {
            Ok(Message::text("cached"))
        }
    }

    fn issuing() -> ConformityConfig {
        ConformityConfig::new().with_conversation_token(true)
    }

    /// A request carrying one `conversation` token the peer owns, and nothing else.
    fn asking(token: &str) -> Message {
        Message::text("hi").with_submessage(Submessage::token("conversation", token))
    }

    /// Every token in `message`, as text, the first submessage included.
    fn tokens(message: &Message) -> Vec<String> {
        message
            .tokens()
            .filter_map(Submessage::content_as_str)
            .map(str::to_string)
            .collect()
    }

    fn no_layers() -> Vec<Arc<dyn DynMiddleware>> {
        Vec::new()
    }

    // ---------------------------------------------------------------------------
    // The failure rules. No executor anywhere below: they are functions over values,
    // which is the test that decides what belongs in this crate at all.
    // ---------------------------------------------------------------------------

    #[test]
    fn only_a_body_that_never_decoded_owes_nothing_and_only_the_ceiling_owes_control() {
        let parse_error = Message::from_json_bytes(b"not json").expect_err("not a message");
        assert_eq!(owed(&Outcome::Malformed(parse_error)), Owed::Nothing);
        assert_eq!(
            owed(&Outcome::Refused(Rejected::TooManyTokens {
                count: 40,
                limit: 32
            })),
            Owed::ControlOnly
        );
        assert_eq!(
            owed(&Outcome::Refused(Rejected::Violations(Vec::new()))),
            Owed::Clause6
        );
        assert_eq!(
            owed(&Outcome::Failed(EndpointError::Internal("x".into()))),
            Owed::Clause6
        );
        // The success path owes the same thing a refusal does, which is why `close`
        // runs there too.
        assert_eq!(owed(&Outcome::Ok), Owed::Clause6);
    }

    #[test]
    fn what_a_peer_is_told_and_what_it_is_asked_to_wait() {
        assert!(Outcome::Ok.public_message().is_none());
        assert!(Outcome::Ok.retry_after().is_none());

        let parse_error = Message::from_json_bytes(b"not json").expect_err("not a message");
        let malformed = Outcome::Malformed(parse_error);
        assert!(
            malformed
                .public_message()
                .is_some_and(|text| text.starts_with("malformed NLIP message: ")),
        );

        let refused = Outcome::Refused(Rejected::TooManyTokens {
            count: 40,
            limit: 32,
        });
        assert_eq!(
            refused.public_message().as_deref(),
            Some("message carries 40 token submessages; this endpoint echoes at most 32"),
            "a refusal describes the peer's own message, which the peer already has"
        );

        // ECMA-434 §7.3.3: an internal detail can leak state, so the peer gets none.
        let internal = Outcome::Failed(EndpointError::Internal("postgres://user:hunter2@db".into()));
        assert!(!internal.public_message().unwrap().contains("hunter2"));

        let delayed = Outcome::Failed(EndpointError::Unavailable {
            detail: "the model provider answered HTTP 429".to_string(),
            retry_after: Some(Duration::from_secs(12)),
        });
        assert_eq!(delayed.retry_after(), Some(Duration::from_secs(12)));
        assert!(internal.retry_after().is_none(), "nothing to promise");
    }

    #[test]
    fn a_refusal_echoes_confirms_and_carries_without_an_endpoint_in_sight() {
        // `()` as the end-point: the failure rules never ask one anything, and saying
        // so in the type is the cheapest way to prove it.
        let conformity = Conformity::new((), Memory::holding("srv-1"), issuing(), Rejection::default());
        let mut request = asking("peer-1");
        request.control = Some(true);
        let cx = Context::new();

        let reply = error_reply(&conformity, Some(&request), "no", Owed::Clause6, &cx);

        assert_eq!(reply.message_type.as_deref(), Some("Error"));
        assert_eq!(reply.content_as_str(), Some("no"));
        // §6.2's echo, §6.2.1's confirmation, §6.3's carry — the three `close` applies.
        assert_eq!(tokens(&reply), ["peer-1", "srv-1"]);
        assert_eq!(reply.control, Some(true));
    }

    #[test]
    fn the_ceiling_carries_control_and_echoes_nothing_at_all() {
        let conformity = Conformity::new((), Memory::holding("srv-1"), issuing(), Rejection::default());
        let mut request = asking("peer-1");
        request.control = Some(true);
        let cx = Context::new();

        let reply = error_reply(&conformity, Some(&request), "too many", Owed::ControlOnly, &cx);

        // Cloning what it refused is the work the ceiling exists to refuse, and not
        // even the confirmation rides along: §6.3 by hand is the whole of it.
        assert!(tokens(&reply).is_empty());
        assert_eq!(reply.control, Some(true));
    }

    #[test]
    fn a_body_that_never_decoded_has_nothing_to_echo_from() {
        let conformity = Conformity::new((), Memory::holding("srv-1"), issuing(), Rejection::default());
        let cx = Context::new();

        let reply = error_reply(&conformity, None, "malformed", Owed::Nothing, &cx);

        assert!(reply.submessages.is_empty());
        assert_eq!(reply.control, None);
    }

    // ---------------------------------------------------------------------------
    // The composer. `rt` and `macros` only — the same tokio the `Endpoint`
    // implementation of `Conformity` is tested with, and no timers or I/O drivers.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn bytes_that_are_not_a_message_are_answered_in_the_protocol() {
        let conformity = Arc::new(Conformity::new(Echo, Memory::default(), issuing(), Rejection::default()));
        let mut cx = Context::new();

        let served = serve(&conformity, &no_layers(), b"{", Wire::Json, &mut cx).await;

        assert!(matches!(served.outcome, Outcome::Malformed(_)));
        assert_eq!(served.reply.message_type.as_deref(), Some("Error"));
        assert!(!served.withheld);
        // Nothing decoded, so nothing is owed — not even the token this end-point would
        // otherwise be confirming, because nothing said which conversation this was.
        assert!(served.reply.submessages.is_empty());
        assert!(served.findings.is_empty());
    }

    #[tokio::test]
    async fn the_token_ceiling_refuses_the_request_and_echoes_none_of_it() {
        let conformity = Arc::new(Conformity::new(
            Echo,
            Memory::holding("srv-1"),
            issuing().with_max_tokens_per_message(1),
            Rejection::default(),
        ));
        let mut request = asking("peer-1").with_submessage(Submessage::token("auth", "peer-2"));
        request.control = Some(true);
        let mut cx = Context::new();

        let served = serve_message(&conformity, &no_layers(), &request, &mut cx).await;

        assert!(matches!(
            served.outcome,
            Outcome::Refused(Rejected::TooManyTokens { count: 2, limit: 1 })
        ));
        assert!(tokens(&served.reply).is_empty(), "the ceiling echoes nothing");
        assert_eq!(served.reply.control, Some(true), "§6.3 still applies");
    }

    #[tokio::test]
    async fn a_request_clause_five_refuses_still_confirms_what_this_endpoint_issued() {
        // §6.2.1's confirmation is owed on every reply, refusals included: a client
        // told to send back "whatever came back" would otherwise drop the server's
        // conversation token after one malformed turn.
        let conformity = Arc::new(Conformity::new(
            Echo,
            Memory::holding("srv-1"),
            issuing(),
            // The default: reject on error inbound.
            Rejection::default(),
        ));
        // A `text` first submessage whose content is not a string — §5.1.4, an error.
        let request = Message::new(Format::Text, "english", Content::Bool(true));
        let mut cx = Context::new();

        let served = serve_message(&conformity, &no_layers(), &request, &mut cx).await;

        assert!(matches!(
            served.outcome,
            Outcome::Refused(Rejected::Violations(_))
        ));
        assert_eq!(tokens(&served.reply), ["srv-1"]);
        assert!(
            served
                .outcome
                .public_message()
                .is_some_and(|text| text.contains("not conforming")),
        );
    }

    #[tokio::test]
    async fn an_endpoint_that_cannot_answer_still_owes_the_echo_and_names_its_delay() {
        let conformity = Arc::new(Conformity::new(
            Failing(EndpointError::Unavailable {
                detail: "the model provider is down".to_string(),
                retry_after: Some(Duration::from_secs(12)),
            }),
            Memory::holding("srv-1"),
            issuing(),
            Rejection::default(),
        ));
        let mut cx = Context::new();

        let served = serve_message(&conformity, &no_layers(), &asking("peer-1"), &mut cx).await;

        assert!(matches!(served.outcome, Outcome::Failed(_)));
        assert_eq!(tokens(&served.reply), ["peer-1", "srv-1"]);
        assert_eq!(served.outcome.retry_after(), Some(Duration::from_secs(12)));
        assert!(!served.withheld);
    }

    #[tokio::test]
    async fn a_layer_that_answers_without_the_endpoint_still_owes_clause_six() {
        // The obligation the composer owns: `close` runs over whatever
        // the stack returned, whether or not `Tail` — and therefore `finish` — ever ran.
        let conformity = Arc::new(Conformity::new(Echo, Memory::holding("srv-1"), issuing(), Rejection::default()));
        let layers: Vec<Arc<dyn DynMiddleware>> = vec![Arc::new(Cached)];
        let mut request = asking("peer-1");
        request.control = Some(true);
        let mut cx = Context::new();

        let served = serve_message(&conformity, &layers, &request, &mut cx).await;

        assert!(matches!(served.outcome, Outcome::Ok));
        assert_eq!(served.reply.content_as_str(), Some("cached"));
        assert_eq!(tokens(&served.reply), ["peer-1", "srv-1"]);
        assert_eq!(served.reply.control, Some(true));
        // `withheld` is about the end-point's own answer, and there was none: the
        // layers saw a successful exchange and so did the peer.
        assert!(!served.withheld);
        // `close` never mints, so a path no deployment committed creates no token —
        // the confirmation above is the one the authority already held.
        assert!(served.findings.is_empty());
    }

    #[tokio::test]
    async fn a_reply_clause_five_rejects_is_withheld_and_replaced() {
        let conformity = Arc::new(Conformity::new(
            NonConforming,
            Memory::holding("srv-1"),
            issuing(),
            Rejection::default(),
        ));
        let mut cx = Context::new();

        let served = serve_message(&conformity, &no_layers(), &asking("peer-1"), &mut cx).await;

        // Not a failure: the peer's *request* was fine, and only the binding's status
        // line changes. That is why this is `Ok` with `withheld` beside it.
        assert!(matches!(served.outcome, Outcome::Ok));
        assert!(served.withheld);
        assert_eq!(served.reply.message_type.as_deref(), Some("Error"));
        assert!(
            served
                .reply
                .content_as_str()
                .is_some_and(|text| text.contains("withheld")),
        );
        // The replacement still carries what clause 6 owes.
        assert_eq!(tokens(&served.reply), ["peer-1", "srv-1"]);
        assert!(
            served.findings.rejected().next().is_some(),
            "the operator has to be able to see which rule the end-point broke"
        );
    }

    #[tokio::test]
    async fn the_happy_path_mints_once_and_reports_both_halves_findings_in_order() {
        let conformity = Arc::new(Conformity::new(Mirror, Memory::default(), issuing(), Rejection::default()));
        // A `token` whose content is not a string: ECMA-430 Table 1 calls a token
        // opaque without fixing its type, so this is a *warning* — logged, not refused
        // — on the way in, and again on the way out once the echo has cloned it.
        let request = Message::text("hi").with_submessage(Submessage::new(
            Format::Token,
            "conversation",
            Content::Bool(true),
        ));
        let mut cx = Context::new();

        let served = serve_message(&conformity, &no_layers(), &request, &mut cx).await;

        assert!(matches!(served.outcome, Outcome::Ok));
        assert!(!served.withheld);
        assert_eq!(served.reply.content_as_str(), Some("hi"));
        // §6.2.1: the peer brought nothing this end-point could recognise as a
        // conversation of its own, so one is minted beside the echo.
        assert_eq!(tokens(&served.reply), ["minted-1"]);
        assert_eq!(
            served.reply.submessages.len(),
            2,
            "the peer's token is echoed whatever its content: {:?}",
            served.reply.submessages
        );

        // The request's findings first, then the reply's — one `token-content` warning
        // on each side of the same submessage.
        let rules: Vec<&str> = served
            .findings
            .iter()
            .map(|finding| finding.violation.rule)
            .collect();
        assert_eq!(
            rules,
            ["ecma430-5.3/token-content", "ecma430-5.3/token-content"],
            "check's findings come first, finish's second"
        );
    }
    #[tokio::test]
    async fn a_reply_the_host_finished_is_closed_and_never_finished_a_second_time() {
        // A caller that ran `finish` itself and hands the result to this composer —
        // a binding that owns its own layer stack. What the engine still owes such a
        // reply is `close`, and nothing else. The caller here is a Rust one, which is
        // the only kind there is: `Conformity::finish` is the sole producer of a
        // `Finished`, and the WebAssembly guest composes its layers inside itself.
        let conformity = Conformity::new((), Minting::default(), issuing(), Rejection::default());
        // No token of the peer's: §6.2.1's minting is reachable, which is what makes
        // a second `finish` observable at all.
        let request = Message::text("hi");
        let mut cx = Context::new();

        // The wrapper's own innermost step.
        let finished = conformity.finish(&request, Message::text("hello"), &cx);
        assert_eq!(conformity.authority().minted(), 1, "§6.2.1, once");

        let served = serve_answered(&conformity, &request, &mut cx, async |_cx| Ok(finished)).await;

        assert!(matches!(served.outcome, Outcome::Ok));
        assert_eq!(served.reply.content_as_str(), Some("hello"));
        assert_eq!(tokens(&served.reply), ["minted-1"]);
        assert!(!served.withheld);
        assert_eq!(
            conformity.authority().minted(),
            1,
            "`serve_answered` closes over a finished reply; it does not finish it again"
        );

        // The hazard, spelled out rather than asserted: running `finish` over the same
        // reply a second time mints a second identifier, and the peer would be handed
        // both.
        let twice = conformity.finish(&request, served.reply.clone(), &cx);
        assert_eq!(conformity.authority().minted(), 2);
        assert_eq!(tokens(&twice.message), ["minted-1", "minted-2"]);
    }

    #[tokio::test]
    async fn a_host_answer_that_fails_owes_the_same_echo_the_composer_owes() {
        // The failure rules are `serve_answered`'s too, because `serve_message` is it:
        // one order, reached by two callers.
        let conformity = Conformity::new((), Memory::holding("srv-1"), issuing(), Rejection::default());
        let mut cx = Context::new();

        let served = serve_answered(&conformity, &asking("peer-1"), &mut cx, async |_cx| {
            Err(EndpointError::Internal("the handler threw".into()))
        })
        .await;

        assert!(matches!(served.outcome, Outcome::Failed(_)));
        assert_eq!(tokens(&served.reply), ["peer-1", "srv-1"]);
        assert!(!served.withheld);
        // ECMA-434 §7.3.3: the detail of an internal fault never reaches the peer.
        assert!(
            served
                .reply
                .content_as_str()
                .is_some_and(|text| !text.contains("threw")),
        );
    }
}
