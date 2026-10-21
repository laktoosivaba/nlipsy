// The crate's documentation *is* the README: one text, rendered by both GitHub and
// rustdoc, with the usage example collected as a doctest — so what a reader sees on the
// repository page is what the test suite compiles, and neither can drift from the other
// quietly. The same arrangement as `nlipsy-protocol` and `nlipsy-core`.
//
// `doc_cfg` is a nightly feature and is enabled only under `--cfg docsrs`, which is what
// docs.rs passes (see `[package.metadata.docs.rs]`). It is what puts the "Available on
// crate feature `hyper`" banner on each transport; a stable build never sees the
// attribute. It arrived with the second transport, which is the first item here to be
// behind a feature at all.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]

pub mod record;
pub mod transport;

pub use record::{FileSink, Replay};
// What a build with **neither** transport still has, which is the shape
// `--no-default-features` compiles: ECMA-431 §6 as values, the canonical chat request,
// the header types a builder would take, and the file-backed recorder. A consumer who
// wants only to replay a recording — a test suite, a fixture runner — needs no socket
// and should not compile one.
pub use transport::{
    BuildError, CONVERSATION_SUBFORMAT, CORRELATOR_HEADER, DEFAULT_CONNECT_TIMEOUT, HeaderMap,
    HeaderValue, NLIP_PATH, request,
};

/// The transport over reqwest, verifying against the platform's trust store.
#[cfg(feature = "reqwest")]
#[cfg_attr(docsrs, doc(cfg(feature = "reqwest")))]
pub use transport::{HttpTransport, HttpTransportBuilder};

/// The transport over hyper, verifying against a compiled-in root bundle — the build an
/// Android or an iOS app over FFI can carry.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub use transport::{HyperTransport, HyperTransportBuilder};

/// The seam under that transport: a caller's own way of opening the byte stream —
/// through an in-process tunnel, a proxy, or no network at all — in place of the system
/// resolver and a socket.
#[cfg(feature = "hyper")]
#[cfg_attr(docsrs, doc(cfg(feature = "hyper")))]
pub use transport::{DialFuture, DialStream, DialTarget, DialedStream, Dialer};

// The core is in this crate's public API — both transports implement its `Transport`,
// `FileSink` its `RecordSink`, and every signature here names its `Context`, `Exchange`
// and `TransportError` — so a consumer has to be able to name the same version of it.
// `nlipsy-core` re-exports `http` for exactly this reason, and jsonrpsee's HTTP client
// re-exports `jsonrpsee_types`. Depending on `nlipsy-core` directly stays equally
// supported: this is for the consumer who would rather name one crate.
pub use nlipsy_core;
