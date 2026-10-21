// Feature gating, in one place, copied in shape from jsonrpsee's `core/src/macros.rs`.
//
// The point is not brevity. A gated item needs two attributes that must agree —
// `#[cfg(feature = "…")]` so it compiles, and `#[cfg_attr(docsrs, doc(cfg(…)))]` so
// docs.rs says which feature it came from — and writing them by hand on every item is
// how one of the two goes missing. Then the item is either in the build and
// undocumented, or documented and not in the build, and nothing fails.
//
// These are `macro_rules!` without `#[macro_export]`: they are used through
// `#[macro_use] mod macros;` inside this crate and are not part of its public API.

macro_rules! cfg_feature {
    ($feature:literal, $($item:item)*) => {
        $(
            #[cfg(feature = $feature)]
            #[cfg_attr(docsrs, doc(cfg(feature = $feature)))]
            $item
        )*
    };
}

/// The outbound half: a `Transport`, a `Client`, recorded exchanges.
macro_rules! cfg_client {
    ($($item:item)*) => {
        cfg_feature!("client", $($item)*);
    };
}

/// The inbound half: an `Endpoint` and the clause-6 obligations around it.
macro_rules! cfg_server {
    ($($item:item)*) => {
        cfg_feature!("server", $($item)*);
    };
}
