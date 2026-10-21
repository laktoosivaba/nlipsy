//! `Send` where the target has threads, and nothing where it does not.
//!
//! Every trait in this crate is written in terms of [`MaybeSend`] rather than `Send`,
//! and every boxed future in terms of [`BoxFuture`]. The reason is the target the
//! workspace already checks in CI: `wasm32-unknown-unknown`, where a JavaScript
//! callback is not `Send` and never will be. A core that demanded `Send` would be
//! unimplementable there — which would put the whole of clause 6 out of reach of the
//! package this workspace is shaped for.
//!
//! On every other target `MaybeSend` *is* `Send`, so nothing is given up: a runtime
//! that spawns a handler onto a thread pool still gets the bound it needs, checked at
//! the same place it always was.

use std::future::Future;
use std::pin::Pin;

/// Marker trait: `Send` on a target that has threads, nothing on `wasm32`.
///
/// Copied from jsonrpsee's `core/src/client/mod.rs`, where it serves the same purpose
/// for the same target. Blanket-implemented, so no implementor ever writes it out.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

/// Marker trait: `Send` on a target that has threads, nothing on `wasm32`.
///
/// Copied from jsonrpsee's `core/src/client/mod.rs`, where it serves the same purpose
/// for the same target. Blanket-implemented, so no implementor ever writes it out.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// A boxed future, `Send` on a target that has threads.
///
/// The object-safe shims (`DynEndpoint`, `DynTransport`) return this, because a
/// return-position `impl Future` cannot appear in a `dyn` trait. The `Send` half of
/// the pair is what lets a runtime move the future between threads; the `wasm32` half
/// is what lets a JavaScript promise be one.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed future, `Send` on a target that has threads.
///
/// The object-safe shims (`DynEndpoint`, `DynTransport`) return this, because a
/// return-position `impl Future` cannot appear in a `dyn` trait. The `Send` half of
/// the pair is what lets a runtime move the future between threads; the `wasm32` half
/// is what lets a JavaScript promise be one.
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
