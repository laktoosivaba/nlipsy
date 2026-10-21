//! What a runtime knows about one exchange, and hands to everything below it.
//!
//! [`Context`] is filled by the runtime before the handler runs and is read by the
//! handler and by the transport. It is owned rather than borrowed: a message-level
//! layer takes `&mut Context` and writes into it — jsonrpsee inserts its
//! `ConnectionId` (`server/src/server.rs`) and `IsBatch`
//! (`core/src/client/async_client/mod.rs`) into an `Extensions` the same way — while
//! the leaf handler and the transport take `&Context` and only read.
//!
//! What is deliberately *not* here: a session, a clock, an address type, a header
//! map. ECMA-430 §6.2 makes conversation state optional ("if the end-points are
//! maintaining session state"), so a core that named a `Session` type would have
//! settled for every deployment a question the standard leaves open. A deployment
//! puts its own session — or a cheap handle to it — in [`Context::extensions`] and
//! reads it back in its [`TokenAuthority`](crate::TokenAuthority) and in its handler.

use http::Extensions;

/// The per-exchange value a runtime fills before the handler runs.
///
/// `#[non_exhaustive]`: a binding will want to hand the handler more than three things
/// — a deadline, a declared content type, the profile of ECMA-434 the deployment
/// claims — and adding a field should not break a consumer. Build one with
/// [`Context::new`] and set the public fields, or use the `with_*` shorthands.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Context {
    /// Opaque label for whoever is on the other end — the peer address, in an HTTP
    /// deployment. Opaque on purpose: a label is all the protocol has any use for,
    /// and a `SocketAddr` here would be a claim that every binding has one.
    pub peer: Option<String>,
    /// Identifier for this one request/response pair.
    ///
    /// ECMA-433 §6.1.4 makes it normative over AMQP — "A server agent that receives a
    /// request with a correlation-id **shall** copy the contents … to the
    /// correlation-id field of the response" — while ECMA-431 defines nothing of the
    /// kind, so over HTTP it is a convention (`X-NLIP-Correlator`, in the reference
    /// server this crate was read out of). That is exactly why it lives here rather
    /// than in a transport method signature: one concept, a different spelling per
    /// binding, and the binding is what knows the spelling.
    pub correlator: Option<String>,
    /// Everything a deployment or a layer wants to hand the handler.
    ///
    /// `http::Extensions` is a type map: one value per type, inserted by whoever owns
    /// it and read back by type. It is where a `Session`, a tenant id or an
    /// authenticated principal travels without this crate having to name any of them.
    pub extensions: Extensions,
}

impl Context {
    /// An empty context: no peer, no correlator, no extensions.
    pub fn new() -> Context {
        Context::default()
    }

    /// Set the peer label.
    pub fn with_peer(mut self, peer: impl Into<String>) -> Context {
        self.peer = Some(peer.into());
        self
    }

    /// Set the correlator.
    pub fn with_correlator(mut self, correlator: impl Into<String>) -> Context {
        self.correlator = Some(correlator.into());
        self
    }

    /// The peer label, or `""` when the runtime did not name one.
    ///
    /// For the common case of writing it into a log line, where an `Option` in a
    /// format string is noise.
    pub fn peer_or_empty(&self) -> &str {
        self.peer.as_deref().unwrap_or_default()
    }

    /// The correlator, or `""` when the runtime did not mint one.
    pub fn correlator_or_empty(&self) -> &str {
        self.correlator.as_deref().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deployments_own_state_travels_in_extensions() {
        // The property the whole design turns on: a type this crate has never heard
        // of reaches the handler, and the core neither names it nor copies it.
        #[derive(Debug, Clone, PartialEq)]
        struct Session(&'static str);

        let mut cx = Context::new().with_peer("10.0.0.1").with_correlator("c-1");
        cx.extensions.insert(Session("s-1"));

        assert_eq!(cx.peer_or_empty(), "10.0.0.1");
        assert_eq!(cx.correlator_or_empty(), "c-1");
        assert_eq!(cx.extensions.get::<Session>(), Some(&Session("s-1")));
    }

    #[test]
    fn an_empty_context_renders_as_empty_strings() {
        let cx = Context::new();
        assert_eq!(cx.peer_or_empty(), "");
        assert_eq!(cx.correlator_or_empty(), "");
    }
}
