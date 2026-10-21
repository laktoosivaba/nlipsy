// The crate's documentation *is* the README, as it is for every other member: one text,
// rendered by both GitHub and rustdoc. Its one code block is `text` rather than `rust`,
// because what it shows is a build recipe for a wasm target and a doctest cannot run
// one.
#![doc = include_str!("../README.md")]
// The workspace *forbids* `unsafe_code`, so that no member can opt back in with an
// attribute, and this member still cannot keep that: every canonical-ABI shim
// `wit_bindgen::generate!` writes is `unsafe`, and a proc macro's tokens carry this
// crate's spans, so the lint fires on generated code. So the crate manifest omits the
// workspace lints and the forbid is re-declared here as a `deny` with **one** exception,
// on the generated bindings below.
//
// One, and only that one: the contract has no resources, so there is no handle, no
// reborrow and no hand-written `unsafe` anywhere in this crate.
#![deny(unsafe_code)]

/// The interface contract, as Rust.
///
/// `wit/nlipsy.wit` is read from the repository root rather than copied here, so the
/// contract and its guest cannot drift: the file the world is generated from is the file
/// `make check-wit` resolves and the npm package's `jco transpile` maps.
///
/// **`generate_all`, and not a `with:` map.** The world reaches `wasi:http@0.3.0`, which
/// pulls `wasi:clocks/types@0.3.0` in transitively, and wit-bindgen otherwise stops with
/// `missing one of: * generate_all option * with: { … }` because it assumes WASI
/// packages come from a crate. The alternative is a
/// `with:` map onto `wasip3`'s pregenerated bindings, which would make this crate depend
/// on a crate for bindings it can generate itself from the WIT that is already in the
/// tree: the *target* decides what `std` links, `wit/deps/` decides what the component
/// asks for, and neither needs `wasip3` as a dependency.
///
/// **No `async:` option.** wit-bindgen honours the WIT's own `async func` marker per
/// function, and the macro's `async` option is an `AsyncFilterSet` whose `true` means
/// *all* — which would lift `decode` through the asynchronous canonical ABI too.
#[allow(unsafe_code)]
pub(crate) mod bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "nlipsy",
        generate_all,
    });

    // The canonical-ABI entry points, on the target that has a canonical ABI.
    //
    // Gated, and the reason is a linker rather than a design: every symbol the macro
    // writes is an `export_name` of the form `nlipsy:engine/protocol@0.1.0#decode`, and
    // Mach-O's `-exported_symbols_list` reads `#` as the start of a comment — so a
    // *native* `cdylib` is asked to export `nlipsy:engine/protocol@0.1.0`, which nothing
    // defines, and fails to link. Nothing is lost: the host build exists to run this
    // crate's unit tests through the `rlib`, where no canonical-ABI entry point is
    // called, and the `Guest` implementations behind these symbols are ordinary trait
    // implementations that are type-checked on both targets.
    #[cfg(target_family = "wasm")]
    use super::Component;

    #[cfg(target_family = "wasm")]
    export!(Component);
}

mod client;
mod convert;
mod engine;
mod host;
mod http;
mod protocol;

/// The guest: one type implementing all four exported interfaces.
///
/// It holds nothing, and under the contract there is nothing it *could*
/// hold: every export is a free function, the host's five interfaces are free functions
/// too, and the only thing that outlives a call is the exchange counter — a `static`
/// atomic in [`host`], documented there as naming an exchange within one instance and
/// nowhere else.
#[derive(Debug)]
pub(crate) struct Component;
