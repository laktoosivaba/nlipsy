// The crate's documentation *is* the README: one text, rendered by both GitHub and
// rustdoc, with the usage example collected as a doctest — so what a reader sees on the
// repository page is what the test suite compiles, and neither can drift from the other
// quietly. The same arrangement as `nlipsy-protocol`, `nlipsy-core`,
// `nlipsy-http-client` and `nlipsy-server`.
#![doc = include_str!("../README.md")]

pub mod binding;
pub mod message;
pub mod retry;
pub mod status;

pub use binding::{
    CONTENT_TYPE, CORRELATOR_HEADER, DEFAULT_MAX_REQUEST_BODY_SIZE, NLIP_PATH, correlator_header,
};
pub use message::{CONVERSATION_SUBFORMAT, method_not_allowed, request, too_large};
pub use retry::{parse_retry_after, retry_after_value};
pub use status::StatusMap;

// `StatusCode`, `HeaderName` and `HeaderValue` are this crate's whole vocabulary of
// HTTP, and every one of them is in its public API, so a consumer has to be able to
// name the same version. `nlipsy-core` re-exports the same crate for `Extensions`, and
// jsonrpsee re-exports it for both reasons.
pub use http;

// The clause-6 vocabulary `StatusMap::of` maps — `Outcome`, `EndpointError` — is
// `nlipsy-core`'s and appears in this crate's public API. Re-exported for the same
// reason `http` is; depending on `nlipsy-core` directly stays equally supported.
pub use nlipsy_core;
