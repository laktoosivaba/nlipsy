//! What each refusal looks like on the status line.
//!
//! `StatusMap` lives in [`nlipsy-http`](nlipsy_http) now, with the rest of ECMA-431 §6
//! as values: the *client* binding reads a status and the *server* binding writes one,
//! and a map that lived in only one of them would be a rule this workspace states twice.
//! It is re-exported here so that a consumer who named this crate keeps the path it
//! already wrote, exactly as `axum` and `nlipsy-core` are re-exported from the root.
//!
//! What changed with the move is `of`. It takes an [`Outcome`](nlipsy_core::Outcome) —
//! the whole way an exchange ended, `withheld` beside it — rather than a bare
//! [`EndpointError`](nlipsy_core::EndpointError), because the composer in `nlipsy-core`
//! reports all four outcomes and a binding maps all four. The old mapping is
//! [`StatusMap::of_error`], which is what the out-of-band upload leg still uses: that
//! leg has no clause 6 around it and therefore no outcome, only a sink that failed.

pub use nlipsy_http::StatusMap;
