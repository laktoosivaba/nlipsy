//! Message-level layers: what a runtime puts between clause 6 and the endpoint.
//!
//! A binding does three kinds of work around one exchange. Reading bytes, choosing a
//! status code and setting headers is the binding's own and stays there. The
//! obligations of clause 6 are [`Conformity`](crate::Conformity)'s and are applied
//! underneath everything — and, because a layer can answer without ever reaching what
//! is underneath it, again over whatever this stack returned, through
//! [`Conformity::close`](crate::Conformity::close): nothing a layer does can remove
//! what clause 6 owes the peer. A layer can still break clause 5 in its after-half,
//! and the outbound policy does not see it. In between sits work that is
//! neither — admitting a conversation, bounding how long the endpoint may take,
//! writing an authenticated principal into the context — which belongs to the
//! *deployment* and has to run after the message is decoded and before the endpoint
//! is asked anything.
//!
//! [`Middleware`] is that seam, and it is deliberately not `tower::Service`. The
//! reasons are jsonrpsee's, whose `RpcServiceT` exists for the same reason
//! (`core/src/middleware/mod.rs`: "similar to `tower::Service` but it's specialized
//! for JSON-RPC operations"): `poll_ready` has nothing to gate when there is one
//! handler at the bottom, `Service::call` takes `&mut self` where a shared endpoint
//! wants `&self`, and a domain trait can hand the layer the request *and* the
//! context by reference instead of moving them into a future.
//!
//! The one thing this trait adds over jsonrpsee's is `&mut Context`. That is the
//! whole point of a message-level layer here: §6.2's exception is answered by a
//! [`TokenAuthority`](crate::TokenAuthority) that reads the deployment's session out
//! of [`Context::extensions`](crate::Context::extensions), so *something* has to put
//! it there, after decoding and before the endpoint runs. jsonrpsee writes into an
//! `Extensions` in the same place and for the same reason (its `ConnectionId`, at
//! `server/src/server.rs`).
//!
//! # What a layer looks like
//!
//! ```
//! use nlipsy_core::{Context, EndpointError, Middleware, Next};
//! use nlipsy_protocol::message::Message;
//!
//! /// Hands the endpoint a tenant, and counts what it answered.
//! struct Tenant(String);
//!
//! #[derive(Clone)]
//! struct TenantId(String);
//!
//! impl Middleware for Tenant {
//!     async fn call(
//!         &self,
//!         request: &Message,
//!         cx: &mut Context,
//!         next: Next<'_>,
//!     ) -> Result<Message, EndpointError> {
//!         // Before: everything the endpoint is allowed to know.
//!         cx.extensions.insert(TenantId(self.0.clone()));
//!         // The rest of the stack, and the endpoint at the bottom of it.
//!         let reply = next.run(request, cx).await?;
//!         // After: only reached when the exchange succeeded, because `?` above
//!         // hands a failure straight back to the binding.
//!         Ok(reply)
//!     }
//! }
//! ```

use std::future::Future;
use std::sync::Arc;

use nlipsy_protocol::message::Message;

use crate::context::Context;
use crate::endpoint::{DynEndpoint, EndpointError};
use crate::send::{BoxFuture, MaybeSend};

/// One layer of the message-level stack.
///
/// `&self` rather than `&mut self`, like [`Endpoint`](crate::Endpoint) and for the
/// same reason: a layer is shared and answers several exchanges at once, so state
/// inside one lives behind whatever lock or atomic it needs.
///
/// A layer that returns an [`EndpointError`] without calling
/// [`Next::run`] refuses the exchange; the binding maps the error to whatever its
/// transport says refusal is. That is how a capacity limit or an authentication
/// failure is written.
///
/// A layer may equally return `Ok(..)` without calling [`Next::run`] — a cache, a
/// canned answer, a maintenance-mode layer — and may change the reply it got back.
/// Both are supported and neither reaches [`Conformity::finish`](crate::Conformity)
/// with the reply that goes on the wire, which is why a binding runs
/// [`Conformity::close`](crate::Conformity::close) over whatever this stack returned:
/// the §6.2 echo, the §6.2.1 confirmation and the §6.3 carry are restored afterwards.
/// Clause 5 is not: a layer that breaks the message model in its after-half has
/// produced a reply the outbound policy never sees, and that is its deployment's bug.
pub trait Middleware: MaybeSend + Sync + 'static {
    /// Answer one exchange, calling [`Next::run`] to continue into the rest of the
    /// stack.
    ///
    /// The lifetimes are all one lifetime on purpose: the request, the context and
    /// the remaining stack are borrowed for as long as the returned future lives,
    /// which is what lets a layer read the request *after* the endpoint has answered
    /// — the property §6.2's echo needs and `tower::Service` cannot give.
    fn call<'a>(
        &'a self,
        request: &'a Message,
        cx: &'a mut Context,
        next: Next<'a>,
    ) -> impl Future<Output = Result<Message, EndpointError>> + MaybeSend + 'a;
}

/// The object-safe form of [`Middleware`].
///
/// A stack is a list of layers of different types, so it is a list of trait objects,
/// so the trait in the list cannot have a method returning `impl Future`. This is the
/// same shim [`DynEndpoint`] is to [`Endpoint`](crate::Endpoint), with the same
/// blanket implementation and the same cost: one allocation per layer per exchange,
/// on a path that already allocated a decoded message.
pub trait DynMiddleware: MaybeSend + Sync + 'static {
    /// Answer one exchange, as a boxed future.
    fn call_boxed<'a>(
        &'a self,
        request: &'a Message,
        cx: &'a mut Context,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Message, EndpointError>>;
}

impl<T: Middleware> DynMiddleware for T {
    fn call_boxed<'a>(
        &'a self,
        request: &'a Message,
        cx: &'a mut Context,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Message, EndpointError>> {
        Box::pin(Middleware::call(self, request, cx, next))
    }
}

/// The rest of the stack, and the endpoint underneath it.
///
/// Handed to a layer so it can continue; dropping it instead of calling
/// [`run`](Next::run) is how a layer refuses an exchange without the endpoint ever
/// seeing it.
pub struct Next<'a> {
    rest: &'a [Arc<dyn DynMiddleware>],
    inner: &'a dyn DynEndpoint,
}

impl<'a> Next<'a> {
    /// The whole stack, with `inner` at the bottom.
    ///
    /// Called by a binding once per exchange. `layers` is outermost first, which is
    /// the order they are read in and the order they were added in.
    pub fn new(layers: &'a [Arc<dyn DynMiddleware>], inner: &'a dyn DynEndpoint) -> Next<'a> {
        Next {
            rest: layers,
            inner,
        }
    }

    /// The endpoint at the bottom of the stack, for a layer that wants to name it in
    /// a log line.
    pub fn endpoint(&self) -> &'a dyn DynEndpoint {
        self.inner
    }

    /// Run the next layer, or the endpoint when there is none left.
    ///
    /// Boxed rather than `impl Future`, because this is the recursive step: a stack
    /// of *n* layers is *n* nested futures whose type would otherwise have to be
    /// written down, and it cannot be — each layer's future names the next one's.
    /// jsonrpsee reaches the same place from the other direction, with its layers
    /// composed as nested generic services and the whole tower boxed once at the
    /// server's edge.
    pub fn run<'b>(
        self,
        request: &'b Message,
        cx: &'b mut Context,
    ) -> BoxFuture<'b, Result<Message, EndpointError>>
    where
        'a: 'b,
    {
        match self.rest.split_first() {
            Some((layer, rest)) => layer.call_boxed(
                request,
                cx,
                Next {
                    rest,
                    inner: self.inner,
                },
            ),
            // The endpoint only *reads* the context: everything that writes to it has
            // had its turn by now, which is the guarantee that makes a layer the place
            // to put a session and the handler the place to read one.
            None => self.inner.handle_boxed(request, cx),
        }
    }
}

impl std::fmt::Debug for Next<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Next")
            .field("remaining_layers", &self.rest.len())
            .field("endpoint", &self.inner.name())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::Endpoint;

    #[derive(Debug, Clone, PartialEq)]
    struct Mark(Vec<&'static str>);

    struct Echo;

    impl Endpoint for Echo {
        fn name(&self) -> &str {
            "echo"
        }

        async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
            // Proof that what the layers wrote is what the endpoint reads.
            let seen = cx.extensions.get::<Mark>().cloned().unwrap_or(Mark(vec![]));
            let mut reply = Message::text(request.content_as_str().unwrap_or_default());
            reply.label = Some(seen.0.join(","));
            Ok(reply)
        }
    }

    struct Note(&'static str);

    impl Middleware for Note {
        async fn call(
            &self,
            request: &Message,
            cx: &mut Context,
            next: Next<'_>,
        ) -> Result<Message, EndpointError> {
            let mut mark = cx.extensions.get::<Mark>().cloned().unwrap_or(Mark(vec![]));
            mark.0.push(self.0);
            cx.extensions.insert(mark);
            let mut reply = next.run(request, cx).await?;
            // The reply is still the layer's to change on the way out.
            let text = format!("{}+{}", reply.content_as_str().unwrap_or_default(), self.0);
            reply.content = nlipsy_protocol::content::Content::String(text);
            Ok(reply)
        }
    }

    struct Refuse;

    impl Middleware for Refuse {
        async fn call(
            &self,
            _request: &Message,
            _cx: &mut Context,
            _next: Next<'_>,
        ) -> Result<Message, EndpointError> {
            Err(EndpointError::Unsupported("nothing gets through".to_string()))
        }
    }

    #[tokio::test]
    async fn layers_run_outermost_first_and_unwind_innermost_first() {
        let layers: Vec<Arc<dyn DynMiddleware>> = vec![Arc::new(Note("outer")), Arc::new(Note("inner"))];
        let echo = Echo;
        let mut cx = Context::new();

        let reply = Next::new(&layers, &echo)
            .run(&Message::text("hi"), &mut cx)
            .await
            .unwrap();

        // The endpoint saw both marks, in the order the layers were added.
        assert_eq!(reply.label.as_deref(), Some("outer,inner"));
        // And the unwinding is the mirror image: the inner layer edits the reply
        // first, the outer one last.
        assert_eq!(reply.content_as_str(), Some("hi+inner+outer"));
    }

    #[tokio::test]
    async fn a_layer_that_refuses_never_reaches_the_endpoint() {
        let layers: Vec<Arc<dyn DynMiddleware>> = vec![Arc::new(Refuse), Arc::new(Note("never"))];
        let echo = Echo;
        let mut cx = Context::new();

        let error = Next::new(&layers, &echo)
            .run(&Message::text("hi"), &mut cx)
            .await
            .unwrap_err();

        assert!(matches!(error, EndpointError::Unsupported(_)));
        assert!(
            cx.extensions.get::<Mark>().is_none(),
            "the layer below the one that refused never ran"
        );
    }

    #[tokio::test]
    async fn an_empty_stack_is_the_endpoint_itself() {
        let layers: Vec<Arc<dyn DynMiddleware>> = Vec::new();
        let echo = Echo;
        let mut cx = Context::new();

        let reply = Next::new(&layers, &echo)
            .run(&Message::text("hi"), &mut cx)
            .await
            .unwrap();
        assert_eq!(reply.content_as_str(), Some("hi"));
    }
}
