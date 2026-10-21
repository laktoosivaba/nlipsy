// The crate's documentation *is* the README: one text, rendered by both GitHub and
// rustdoc, with the usage example collected as a doctest — so what a reader sees on the
// repository page is what the test suite compiles, and neither can drift from the other
// quietly. The same arrangement as `nlipsy-protocol`, `nlipsy-core` and
// `nlipsy-http-client`.
#![doc = include_str!("../README.md")]

pub mod reply;
pub mod server;
pub mod status;
pub mod upload;

pub use reply::{CONTENT_TYPE, reject};
pub use server::{
    CORRELATOR_HEADER, DEFAULT_MAX_REQUEST_BODY_SIZE, MAX_CORRELATOR_LENGTH, MAX_PRINCIPAL_LENGTH,
    NLIP_PATH, Peer, Principal, Server, ServerBuilder, ServerConfig,
};
pub use status::StatusMap;
pub use upload::{DiscardSink, Upload, UploadSink};

// `axum::Router` is the return type of `Server::router`, so a deployment has to be
// able to name the same version of axum this crate built it with — one `Router` from
// two majors is two unrelated types, and the error a mismatch produces says nothing
// about the version. `jsonrpsee-server` re-exports `tokio` for exactly this reason,
// and `nlipsy-http-client` re-exports `nlipsy-core`. Depending on axum directly stays
// equally supported: this is for the consumer who would rather name one crate.
pub use axum;

// Clause 6 itself, the handler seam and the layer seam are `nlipsy-core`'s and appear
// throughout this crate's public API — `Server::new` takes an `Endpoint` and a
// `TokenAuthority`, `ServerConfig` carries a `ConformityConfig` and a `Rejection`,
// `StatusMap` maps an `EndpointError`. Re-exported for the same reason axum is.
pub use nlipsy_core;
