# The WASI interfaces this contract depends on

Vendored, not written here. `wit/nlipsy.wit`'s world exports `wasi:http/handler@0.3.0`
and imports `wasi:http/client@0.3.0` and `wasi:random/random@0.3.0`, so the resolver
needs those packages on disk: `wasm-tools component wit wit/` reads this directory, and
so does `wit_bindgen::generate!({ path: "../../wit", … })` in `crates/nlipsy-component`,
which is why the guest needs no `wasip3` crate dependency at all — the *target* decides
what `std` links, this directory decides what the component asks for.

| what | value |
|---|---|
| source | the `wasip3` crate's `wit/deps/`, byte for byte |
| version | `wasip3` **0.9.0+wasi-0.3.0** (crates.io), which carries `package wasi:http@0.3.0;` |
| upstream | <https://github.com/bytecodealliance/wasi-rs> |
| licence | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` — the crate's own, which this workspace's `MIT OR Apache-2.0` can carry |
| copied on | 2026-09-16 |

```sh
cargo fetch   # or: cargo add wasip3@0.9 --dry-run, to make the registry hold it
cp ~/.cargo/registry/src/*/wasip3-0.9.0+wasi-0.3.0/wit/deps/*.wit wit/deps/
```

## Why all six and not two

The world reaches `wasi:http@0.3.0` and `wasi:random@0.3.0`. `http.wit` uses
`wasi:clocks/types@0.3.0` for its `duration`, and the `proxy` world inside it imports
`wasi:cli/types@0.3.0`; `cli.wit`'s own worlds then import `wasi:filesystem` and
`wasi:sockets`. A resolver reads every declaration in a file it loads, world or not, so
the chain has to be complete or nothing resolves — the first attempt with `http.wit`,
`clocks.wit` and `random.wit` alone failed with `package 'wasi:cli@0.3.0' not found`.

Nothing is lost by carrying the other three. wit-bindgen generates code for the
resolved *world*, not for the directory, so `filesystem` and `sockets` contribute no
bindings and no bytes: the stub crate `README.md` §2 builds over this directory is 177 606 bytes and
its import list is the fifteen `wasi:cli`/`wasi:clocks` interfaces `std` links plus
`wasi:http/types` and `wasi:clocks/types`.

## What must not happen to these files

They are copies of someone else's interface and are not edited — not to trim a world,
not to fix a comment. A version bump is a fresh copy of all six from the same crate
version, with the table above updated and `make check-wit` re-run. The seed rule of
`../README.md` §1 does not reach here: these are not nlipsy's items and have no seed in
this workspace, which is also why `check-seeds.mjs` reads `nlipsy.wit` and `README.md`
§3 and nothing else.
