//! §5.1.1's two-way split, and the whole of NLIP's routing.
//!
//! JSON-RPC needs a method table because a request names a method. NLIP does not: a
//! message is either a control message or it is not (§5.1.1, and §6.3 gives control
//! exchanges their own obligation), so the only branch the protocol asks for is a
//! two-way one. [`Dispatcher`] is that branch and nothing else — no name table, no
//! registration, no pattern matching on content.

use std::sync::Arc;

use nlipsy_protocol::message::Message;

use crate::endpoint::DynEndpoint;

// Only the `Endpoint` implementation below needs these, and that implementation is
// not compiled for `wasm32` — see its own documentation for why. Gated with it rather
// than left unused, so the one target this workspace checks a second time stays
// warning-free.
#[cfg(not(target_arch = "wasm32"))]
use crate::context::Context;
#[cfg(not(target_arch = "wasm32"))]
use crate::endpoint::{Endpoint, EndpointError};

/// Routes control messages to one endpoint and everything else to another.
///
/// When no control endpoint is configured, control requests reach the default one —
/// which is correct behaviour rather than a degraded one: an endpoint that answers
/// every control request with a data reply still satisfies §6.3's control-echo rule,
/// because the echo is applied by [`Conformity`](crate::Conformity) around whichever
/// endpoint answered.
///
/// It holds [`DynEndpoint`] rather than [`Endpoint`] because the two slots are
/// usually filled from configuration, and a trait whose method returns `impl Future`
/// has no `dyn` form. The blanket implementation in [`endpoint`](crate::endpoint)
/// makes every endpoint one.
#[derive(Clone)]
pub struct Dispatcher {
    default: Arc<dyn DynEndpoint>,
    control: Option<Arc<dyn DynEndpoint>>,
}

impl Dispatcher {
    /// A dispatcher with both slots filled, or with only the data slot.
    pub fn new(default: Arc<dyn DynEndpoint>, control: Option<Arc<dyn DynEndpoint>>) -> Dispatcher {
        Dispatcher { default, control }
    }

    /// One endpoint for everything, control messages included.
    pub fn single(endpoint: Arc<dyn DynEndpoint>) -> Dispatcher {
        Dispatcher::new(endpoint, None)
    }

    /// The endpoint that answers this message.
    pub fn route(&self, message: &Message) -> &dyn DynEndpoint {
        match (&self.control, message.is_control()) {
            (Some(control), true) => control.as_ref(),
            _ => self.default.as_ref(),
        }
    }

    /// The names of the endpoints behind this dispatcher, for a status page or a log
    /// line.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        [Some(self.default.as_ref()), self.control.as_deref()]
            .into_iter()
            .flatten()
            .map(DynEndpoint::name)
    }
}

/// A dispatcher is itself an endpoint: route, then hand the message on.
///
/// Which is what lets [`Conformity`](crate::Conformity) wrap the whole two-way split
/// rather than each half of it, so §6.2 and §6.3 apply to a control reply exactly as
/// they do to a data one.
///
/// **Not on `wasm32`**, and the exclusion is the same asymmetry
/// `impl Endpoint for Arc<T>` spells as a `where` clause. [`Endpoint`] requires `Sync`
/// on every target; `Arc<T>: Sync` requires `T: Send`; and on `wasm32`
/// [`MaybeSend`](crate::MaybeSend) is deliberately not `Send`, so
/// `dyn DynEndpoint` is not `Send` and an `Arc` of one is not `Sync`. A `where` clause
/// cannot express that here because the type is concrete — a bound naming no type
/// parameter is checked at the definition and fails outright — so it is a `cfg`. The
/// consequence for a single-threaded target is the one already documented for
/// `Arc<dyn DynEndpoint>`: choose the endpoint concretely, or share it some other way.
/// The *routing rule* — [`route`](Dispatcher::route) — is available everywhere, which
/// is the half that is protocol rather than plumbing.
#[cfg(not(target_arch = "wasm32"))]
impl Endpoint for Dispatcher {
    /// The default endpoint's name. There is no single name for a two-way split, and
    /// the data endpoint is what a deployment is *for*; both are reported by
    /// [`Dispatcher::names`].
    fn name(&self) -> &str {
        // Spelled out because an `Arc<dyn DynEndpoint>` implements both traits and
        // both have a `name`. They agree; the compiler has no way to know that.
        DynEndpoint::name(self.default.as_ref())
    }

    async fn handle(&self, request: &Message, cx: &Context) -> Result<Message, EndpointError> {
        self.route(request).handle_boxed(request, cx).await
    }
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("default", &DynEndpoint::name(self.default.as_ref()))
            .field("control", &self.control.as_deref().map(DynEndpoint::name))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::endpoint::{Endpoint, EndpointError};

    struct Named(&'static str);

    impl Endpoint for Named {
        fn name(&self) -> &str {
            self.0
        }

        async fn handle(&self, _request: &Message, _cx: &Context) -> Result<Message, EndpointError> {
            Ok(Message::text(self.0))
        }
    }

    #[test]
    fn control_requests_reach_the_control_endpoint() {
        let dispatcher = Dispatcher::new(Arc::new(Named("data")), Some(Arc::new(Named("control"))));

        let control = Message::text("?").message_type("Control");
        assert_eq!(dispatcher.route(&control).name(), "control");

        let data = Message::text("hello");
        assert_eq!(dispatcher.route(&data).name(), "data");
    }

    #[test]
    fn without_a_control_endpoint_everything_goes_to_the_default_one() {
        let dispatcher = Dispatcher::single(Arc::new(Named("data")));
        let control = Message::text("?").message_type("Control");
        assert_eq!(dispatcher.route(&control).name(), "data");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn a_dispatcher_is_an_endpoint_in_its_own_right() {
        // What lets the clause-6 wrapper sit outside the whole split rather than
        // inside each half of it.
        let dispatcher = Dispatcher::new(Arc::new(Named("data")), Some(Arc::new(Named("control"))));
        let cx = Context::new();

        let answered = dispatcher
            .handle(&Message::text("?").message_type("Control"), &cx)
            .await
            .unwrap();
        assert_eq!(answered.content_as_str(), Some("control"));
        assert_eq!(Endpoint::name(&dispatcher), "data");
        assert_eq!(
            dispatcher.names().collect::<Vec<_>>(),
            vec!["data", "control"]
        );
    }
}
