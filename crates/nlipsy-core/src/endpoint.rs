//! The seam between the protocol and whatever an endpoint actually does.
//!
//! An [`Endpoint`] sees a decoded request and a [`Context`] and returns a message. It
//! never sees a socket, never sees raw bytes, and — importantly — never touches token
//! submessages: echoing those is the protocol's obligation (§6.2), applied around the
//! endpoint by [`Conformity`](crate::Conformity). Keeping that boundary is what lets a
//! new endpoint be written without re-reading the standard.
//!
//! [`DynEndpoint`] is the same trait made object-safe, because a deployment that
//! chooses its endpoint from configuration needs one value of one type whichever
//! branch it took.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use nlipsy_protocol::message::Message;
use thiserror::Error;

use crate::context::Context;
use crate::send::{BoxFuture, MaybeSend};

/// What an NLIP endpoint does with one message.
///
/// `Sync` as well as [`MaybeSend`] because an endpoint is shared: a runtime holds one
/// behind an `Arc` and answers several requests from it at once, which is the same
/// reason jsonrpsee's `RpcServiceT::call` takes `&self`. On `wasm32` the bound is
/// free — a single-threaded runtime satisfies it — but it is a real bound, and an
/// implementor holding a `!Sync` JavaScript handle would have to put it behind a lock
/// or a thread-local.
///
/// `handle` returns `impl Future` rather than being an `async fn` behind
/// `#[async_trait]`, which costs no allocation on the static path; jsonrpsee made the
/// same move once its MSRV allowed it (`core/src/middleware/mod.rs`).
pub trait Endpoint: MaybeSend + Sync + 'static {
    /// Stable name, for logs and diagnostics.
    fn name(&self) -> &str;

    /// Answer one request.
    ///
    /// Returning a message is the normal path. An [`EndpointError`] describes a
    /// failure to answer; how it reaches the peer — a status code, a frame, an AMQP
    /// property — is the binding's business, and the binding is what maps it.
    ///
    /// **One reply per exchange.** ECMA-433 §6.2's several replies — "responses in
    /// each of the indicated content-types" — are one finished message, rendered by
    /// the AMQP binding once per requested content type and addressed per
    /// `nlip-reply-<content-type>`; the tokens and the control carry are the same in
    /// each. A reply whose *content* differs per format (§6.2's own example: JSON for
    /// a machine and text for a person) is not expressible through this trait and is a
    /// decision for that binding, not a promise of this one.
    fn handle(
        &self,
        request: &Message,
        cx: &Context,
    ) -> impl Future<Output = Result<Message, EndpointError>> + MaybeSend;
}

/// The object-safe form of [`Endpoint`].
///
/// A return-position `impl Trait` cannot appear in a `dyn` trait, so an endpoint
/// chosen at run time — from a configuration switch, from a plugin table — needs this
/// shim. Every [`Endpoint`] is one through the blanket implementation below; the cost
/// is one allocation per call on the `dyn` path and nothing on the static one.
///
/// The future method is named `handle_boxed` rather than `handle` so that a value
/// implementing both traits (an `Arc<dyn DynEndpoint>` does) has no ambiguous call.
pub trait DynEndpoint: MaybeSend + Sync + 'static {
    /// Stable name, for logs and diagnostics.
    fn name(&self) -> &str;

    /// Answer one request, as a boxed future.
    fn handle_boxed<'a>(
        &'a self,
        request: &'a Message,
        cx: &'a Context,
    ) -> BoxFuture<'a, Result<Message, EndpointError>>;
}

impl<T: Endpoint> DynEndpoint for T {
    fn name(&self) -> &str {
        Endpoint::name(self)
    }

    fn handle_boxed<'a>(
        &'a self,
        request: &'a Message,
        cx: &'a Context,
    ) -> BoxFuture<'a, Result<Message, EndpointError>> {
        Box::pin(Endpoint::handle(self, request, cx))
    }
}

// `async fn` rather than returning the boxed future directly: a return-position
// `impl Trait` in a trait *impl* does not capture the method's elided lifetimes, so
// handing back a `BoxFuture<'a, _>` is rejected (E0700). Awaiting it inside an `async`
// block is the same future with one more layer and no allocation of its own, and it is
// what the language gives for exactly this shape.
impl Endpoint for Box<dyn DynEndpoint> {
    fn name(&self) -> &str {
        DynEndpoint::name(&**self)
    }

    async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        DynEndpoint::handle_boxed(&**self, request, cx).await
    }
}

/// A shared endpoint is an endpoint, wherever the sharing is sound.
///
/// The `where` clause is not decoration. `Arc<T>: Sync` requires `T: Send`, which on a
/// native target comes free — [`MaybeSend`] *is* `Send` there, so `dyn DynEndpoint`
/// carries it — and on `wasm32` deliberately does not. Spelling the bound as "whenever
/// the `Arc` itself satisfies this trait's own bounds" is what lets the impl exist on
/// both targets: `Arc<dyn DynEndpoint>` is an `Endpoint` on a threaded runtime, and a
/// single-threaded one shares a `!Send` handler some other way (a `Box`, an `Rc`, or an
/// `Arc<dyn DynEndpoint + Send>` if it really has one).
impl<T: DynEndpoint + ?Sized> Endpoint for Arc<T>
where
    Arc<T>: MaybeSend + Sync,
{
    fn name(&self) -> &str {
        DynEndpoint::name(&**self)
    }

    async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        DynEndpoint::handle_boxed(&**self, request, cx).await
    }
}

/// The endpoint understood the message but could not produce an answer.
///
/// The variants exist because a peer can act differently on each: retrying changes
/// nothing for an unsupported request, may help for an unavailable dependency, and
/// must not be attempted for an internal fault.
///
/// `#[non_exhaustive]`: [`EndpointError::Refused`] was added when the clause-6 wrapper
/// gained a failure of its own, and more bindings will want more. Match with a `_`
/// arm, or use [`public_message`](EndpointError::public_message) and `Display`, which
/// every variant answers.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum EndpointError {
    /// The endpoint does not implement this kind of exchange.
    ///
    /// Reserved for requests it will never answer as written — a payload modality it
    /// cannot interpret, for instance. Retrying changes nothing, which is what
    /// separates this from [`EndpointError::Unavailable`].
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Something the endpoint depends on is down or has asked it to slow down.
    ///
    /// Distinct from [`EndpointError::Internal`] because the peer can act on the
    /// difference: this is transient and a later retry may succeed, whereas an
    /// internal error means retrying the same request is pointless.
    #[error("temporarily unavailable: {detail}")]
    Unavailable {
        /// Safe to show the peer.
        detail: String,
        /// How long the dependency asked us to wait, when it said so.
        ///
        /// A [`Duration`], which is a value. `Retry-After` is one binding's spelling
        /// of it and belongs to that binding.
        retry_after: Option<Duration>,
    },

    /// The endpoint did not answer inside the time a runtime allowed it.
    ///
    /// Data, not a clock: the [`Duration`] is the bound that was exceeded, and
    /// producing this variant is the business of whatever holds the timer — a
    /// message-level layer over an executor, which is outside this crate by
    /// construction. Separate from [`EndpointError::Unavailable`] because the two say
    /// different things and a binding answers them differently: a dependency that
    /// asked for a delay is a 503 with the delay attached, while an endpoint that ran
    /// out of time is a 504 with nothing to promise.
    #[error("timed out after {}s", after.as_secs())]
    TimedOut {
        /// The limit that was exceeded.
        after: Duration,
    },

    /// An internal fault. The detail is logged, never returned to the peer.
    #[error("internal error: {0}")]
    Internal(String),

    /// The request was refused before the endpoint saw it.
    ///
    /// Produced only by [`Conformity`](crate::Conformity), whose
    /// [`check`](crate::Conformity::check) runs before the inner endpoint: the message
    /// broke clause 5 under a policy that rejects, or carried more tokens than the
    /// endpoint is willing to echo back. It is a separate variant because a binding
    /// has to tell it from the three above — the peer's message is at fault, not the
    /// endpoint — and that is the difference between a 4xx and a 5xx.
    #[error("refused: {0}")]
    Refused(#[from] crate::conformity::Rejected),
}

impl EndpointError {
    /// Message text safe to hand back to the calling peer.
    pub fn public_message(&self) -> String {
        match self {
            EndpointError::Unsupported(detail) => format!("unsupported request: {detail}"),
            EndpointError::Unavailable { detail, .. } => {
                format!("the endpoint is temporarily unable to answer: {detail}")
            }
            // Says nothing about the limit: how long this endpoint is willing to
            // spend is a deployment's business, and a peer that learns the number
            // learns how long to hold a request open to occupy a worker.
            EndpointError::TimedOut { .. } => {
                "the endpoint did not answer within its time limit; retry later".to_string()
            }
            // Deliberately opaque: internal details can leak state (ECMA-434 §7.3.3).
            EndpointError::Internal(_) => "the endpoint failed to process the request".to_string(),
            // Not opaque, and for the same reason the others are: this one describes
            // the peer's own message, which the peer already has.
            EndpointError::Refused(rejected) => rejected.to_string(),
        }
    }

    /// The delay the dependency asked for, if it named one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            EndpointError::Unavailable { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str);

    impl Endpoint for Named {
        fn name(&self) -> &str {
            self.0
        }

        async fn handle(&self, _request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
            Ok(Message::text(self.0))
        }
    }

    #[tokio::test]
    async fn a_boxed_endpoint_is_an_endpoint() {
        // What a deployment that chooses its endpoint from configuration needs: one
        // value of one type on either branch, still usable everywhere an `Endpoint`
        // is asked for.
        let boxed: Box<dyn DynEndpoint> = Box::new(Named("configured"));
        let cx = Context::new();

        assert_eq!(Endpoint::name(&boxed), "configured");
        let reply = Endpoint::handle(&boxed, &Message::text("?"), &cx)
            .await
            .unwrap();
        assert_eq!(reply.content_as_str(), Some("configured"));
    }

    #[tokio::test]
    async fn a_shared_endpoint_is_an_endpoint() {
        let shared: Arc<dyn DynEndpoint> = Arc::new(Named("shared"));
        let also = Arc::clone(&shared);
        let cx = Context::new();

        assert_eq!(Endpoint::name(&shared), "shared");
        assert!(
            Endpoint::handle(&also, &Message::text("?"), &cx)
                .await
                .is_ok()
        );
    }

    #[test]
    fn an_internal_fault_says_nothing_about_what_went_wrong() {
        // ECMA-434 §7.3.3: internal detail can leak state, so the peer gets none of
        // it. The other two variants describe the request, which the peer already has.
        let internal = EndpointError::Internal("connection string: postgres://…".to_string());
        assert!(!internal.public_message().contains("postgres"));

        let unsupported = EndpointError::Unsupported("binary/image".to_string());
        assert!(unsupported.public_message().contains("binary/image"));
    }

    #[test]
    fn only_an_unavailable_dependency_names_a_delay() {
        let waiting = EndpointError::Unavailable {
            detail: "the model is rate limiting".to_string(),
            retry_after: Some(Duration::from_secs(7)),
        };
        assert_eq!(waiting.retry_after(), Some(Duration::from_secs(7)));
        assert_eq!(
            EndpointError::Internal("x".to_string()).retry_after(),
            None,
            "retrying an internal fault is pointless, so no delay is suggested"
        );
        assert_eq!(
            EndpointError::TimedOut {
                after: Duration::from_secs(30)
            }
            .retry_after(),
            None,
            "an endpoint that ran out of time has nothing to promise about the next try"
        );
    }
}
