// The crate's documentation *is* the README: one text, rendered by both GitHub and
// rustdoc, with the usage example collected as a doctest — so what a reader sees on
// the repository page is what the test suite compiles and runs, and neither can
// drift from the other quietly.
//
// The intra-doc links that would normally stand at the top of a crate are absent on
// purpose. `[`Message`](message::Message)` resolves for rustdoc and renders as
// literal text on GitHub; a relative markdown link renders on GitHub and 404s on
// docs.rs. No single spelling is correct in both, so the prose names items as plain
// code spans and leaves navigation to rustdoc's own index.
//
// `doc_cfg` is a nightly feature and is enabled only under `--cfg docsrs`, which is
// what docs.rs passes (see `[package.metadata.docs.rs]`). It is what puts the
// "Available on crate feature `cbor`" banner on the CBOR entry points; a stable
// build never sees the attribute.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]

pub mod content;
pub mod error;
pub mod message;

pub use content::{Content, Wire};
pub use error::{ParseError, ParseErrorKind, Severity, Violation};
pub use message::{Format, Message, Submessage};
