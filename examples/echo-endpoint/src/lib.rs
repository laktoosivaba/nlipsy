// The crate's documentation *is* the README, as it is for every other member.
#![doc = include_str!("../README.md")]
// The workspace forbids `unsafe_code` and the generated bindings need it; see the
// manifest. Exactly one exception, on the generated module, and none on anything written
// here.
#![deny(unsafe_code)]

/// The five interfaces this component supplies, as Rust.
///
/// A world of its own, declared inline over `wit/nlipsy.wit`: what the engine *imports*
/// is what an end-point component *exports*, and `wac plug` is what joins the two. The
/// `path` is the contract itself, so this file and the engine cannot disagree about a
/// signature — there is one WIT in the tree and both sides read it.
#[allow(unsafe_code)]
pub(crate) mod bindings {
    wit_bindgen::generate!({
        inline: "
            package nlipsy:echo@0.1.0;

            /// A deployment: a handler, no layers, no tokens, no transport, no recorder.
            world echo-endpoint {
              export nlipsy:engine/host-endpoint@0.1.0;
              export nlipsy:engine/host-layers@0.1.0;
              export nlipsy:engine/host-authority@0.1.0;
              export nlipsy:engine/host-transport@0.1.0;
              export nlipsy:engine/host-record-sink@0.1.0;
            }
        ",
        path: "../../wit",
        world: "echo-endpoint",
        generate_all,
    });

    // The canonical-ABI entry points, on the target that has a canonical ABI. Gated for
    // the reason `crates/nlipsy-component/src/lib.rs` gives: every symbol the macro
    // writes is an `export_name` containing `#`, which Mach-O's exported-symbols list
    // reads as a comment, so a *native* `cdylib` fails to link.
    #[cfg(target_family = "wasm")]
    use super::Endpoint;

    #[cfg(target_family = "wasm")]
    export!(Endpoint);
}

mod convert;
mod host;

/// The deployment: one type implementing all five imported interfaces.
///
/// It holds nothing, because there is nothing to hold: every function in the contract's
/// five `host-*` interfaces is free, and what a deployment with state would key that
/// state on is `context.exchange`, the number the engine hands it.
#[derive(Debug)]
pub(crate) struct Endpoint;
