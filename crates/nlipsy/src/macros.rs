// The `#[cfg]` and the `doc(cfg)` that goes with it, written once.
//
// A feature-gated re-export needs two attributes that say the same thing in two
// languages: `#[cfg]`, which decides whether the item compiles, and `doc(cfg)`, which
// is what puts the "Available on crate feature `protocol`" banner on it for docs.rs.
// Nothing checks that the pair agrees. Writing them apart means a `ws` module added in
// a hurry compiles correctly and documents itself as unconditional, and the mistake is
// invisible until someone reads the rendered page and believes it.
//
// This is jsonrpsee's `cfg_feature!` (jsonrpsee/src/macros.rs) and its
// `cfg_client_or_server!`. jsonrpsee also has a one-line wrapper per *single* feature
// (`cfg_server!`, …); those are left out here, because ours would each be used exactly
// once and a name used once hides the feature it stands for from the reader of the
// item. The three macros below the first one are not that: each stands for an `any(..)`
// over features that produce **one** module between them, which cannot be written as
// `cfg_feature!` twice without defining the module twice.

/// Emit each item behind `$feature`, documented as being behind it.
macro_rules! cfg_feature {
    ($feature:literal, $($item:item)*) => {
        $(
            #[cfg(feature = $feature)]
            #[cfg_attr(docsrs, doc(cfg(feature = $feature)))]
            $item
        )*
    };
}

/// Emit each item when *either* role's core feature is on, documented as such.
///
/// `nlipsy::core` is the one module two features produce, because `nlipsy-core` holds
/// both halves of the runtime and a deployment that federates asks for both. Written
/// as a plain `cfg_feature!` twice it would be emitted twice and the crate would not
/// compile; written as one `#[cfg(any(..))]` by hand it would lose the banner. This is
/// jsonrpsee's `cfg_client_or_server!` with the `doc(cfg)` it omits — ours is worth
/// having, because with no features on there is nothing else on the facade's page at
/// all and the banner is how a reader learns which word to add.
macro_rules! cfg_client_or_server_core {
    ($($item:item)*) => {
        $(
            #[cfg(any(feature = "client-core", feature = "server-core"))]
            #[cfg_attr(
                docsrs,
                doc(cfg(any(feature = "client-core", feature = "server-core")))
            )]
            $item
        )*
    };
}

/// Emit each item when *either* binding feature is on, documented as such.
///
/// `nlipsy::http` is the second module more than one feature produces, and for the same
/// reason `nlipsy::core` is: ECMA-431 §6 as values is what a client agent and a server
/// agent share — one reads the status and the `Retry-After` the other wrote — so
/// `nlipsy-http` arrives with either binding and the module has to be emitted exactly
/// once when both are asked for. Three features now rather than two: the client binding
/// has a transport feature each for reqwest and hyper, and the values are the binding's
/// either way.
macro_rules! cfg_http {
    ($($item:item)*) => {
        $(
            #[cfg(any(
                feature = "http-client",
                feature = "http-client-hyper",
                feature = "server"
            ))]
            #[cfg_attr(
                docsrs,
                doc(cfg(any(
                    feature = "http-client",
                    feature = "http-client-hyper",
                    feature = "server"
                )))
            )]
            $item
        )*
    };
}

/// Emit each item when *either* HTTP client transport is on, documented as such.
///
/// `nlipsy::http_client` is the third such module, and the first whose
/// two features are not two roles: `http-client` is the binding over reqwest and
/// `http-client-hyper` the same binding over hyper, and the crate behind them is one
/// crate either way. The module therefore has to be emitted exactly once when both are
/// asked for — the same failure `cfg_client_or_server_core!` exists to avoid, one module
/// along.
///
/// Why they are two features rather than one with a knob is `nlipsy-http-client`'s
/// story and is written there: reqwest's TLS cannot be built for an Android or an iOS
/// app, and a feature cannot remove a dependency.
macro_rules! cfg_http_client {
    ($($item:item)*) => {
        $(
            #[cfg(any(feature = "http-client", feature = "http-client-hyper"))]
            #[cfg_attr(
                docsrs,
                doc(cfg(any(feature = "http-client", feature = "http-client-hyper")))
            )]
            $item
        )*
    };
}
