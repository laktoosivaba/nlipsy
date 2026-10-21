// The crate's documentation *is* the README: one text, rendered by both GitHub and
// rustdoc, with the usage example collected as a doctest — so what a reader sees on
// the repository page is what the test suite compiles and runs, and neither can
// drift from the other quietly. The same arrangement as `nlipsy-protocol`.
//
// `doc_cfg` is a nightly feature and is enabled only under `--cfg docsrs`, which is
// what docs.rs passes (see `[package.metadata.docs.rs]`). It is what puts the
// "Available on crate feature `server`" banner on the gated halves; a stable build
// never sees the attribute.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]

// Feature gating, used through `#[macro_use]` and not exported.
#[macro_use]
mod macros;

pub mod context;
// Neither half's, because §6.2 ¶2 is symmetric: the endpoint half chains
// `first_submessage_token` in front of `Message::tokens` to build the echo and the
// client half chains it in front of the same iterator to say what came back. A rule
// with one implementation cannot sit behind a feature only one of its two callers
// enables.
pub mod first_submessage;
pub mod send;

pub use context::Context;
pub use first_submessage::first_submessage_token;
pub use send::{BoxFuture, MaybeSend};

// `http` appears in `Context`'s public API through `Extensions`, so a consumer has to
// be able to name the same version of it. jsonrpsee re-exports it for the same reason.
pub use http;

cfg_server! {
    pub mod conformity;
    pub mod dispatch;
    pub mod endpoint;
    pub mod middleware;
    pub mod serve;
    pub mod token;
}

cfg_server! {
    pub use conformity::{
        Conformity, ConformityConfig, DEFAULT_MAX_TOKENS_PER_MESSAGE, Disposition, Finding,
        Findings, Finished, Policy, Rejected, Rejection, token_count,
    };
    pub use dispatch::Dispatcher;
    pub use endpoint::{DynEndpoint, Endpoint, EndpointError};
    pub use middleware::{DynMiddleware, Middleware, Next};
    pub use serve::{
        Outcome, Owed, Served, error_reply, owed, serve, serve_answered, serve_message,
    };
    pub use token::{TokenAuthority, TokenKind};
}

cfg_client! {
    pub mod record;
    pub mod transport;
}

cfg_client! {
    pub use record::{RecordError, RecordSink, Recorded, Recorder, Replay, ReplayError};
    pub use transport::{
        Body, Client, DynTransport, Exchange, ExchangeDetail, OPAQUE_PREVIEW, Reply, Transport,
        TransportError, decode_reply, encode_request,
    };
}

#[cfg(all(feature = "client", feature = "testing"))]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub mod testing;
