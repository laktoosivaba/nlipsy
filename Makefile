# nlipsy checks.
#
#   make check    # check-features, then fmt + clippy + tests
#
# The same three commands a consumer runs over its own workspace, and `--workspace` is
# not optional here. This repository root is a virtual manifest with no package of its
# own, and a bare `cargo test` stops at the root manifest's `default-members` — the
# native crates and the corpus runner — so `--workspace` is what reaches every member,
# and there are nine: nlipsy-protocol is the message model, nlipsy-core is ECMA-430
# clause 6 plus the handler, layer and transport seams and the `serve` composer over
# them, nlipsy-http is ECMA-431 §6 as values shared by both ends of the binding,
# nlipsy-http-client is the ECMA-431 client binding over reqwest or over hyper (two
# transports, two features — `make check-mobile-graph` is why), nlipsy-server is the
# ECMA-431 server binding over axum, nlipsy is the facade that re-exports them behind
# feature flags (all six published), nlipsy-conformance (`crates/conformance`, never
# published) is what runs the corpus under `conformance/` against the model,
# nlipsy-component (never published) is the `wit-bindgen` guest of `wit/nlipsy.wit` — the
# same engine as a WebAssembly component, checked here on the host through its `rlib` and
# built for `wasm32-wasip3` by `make check-component`, and examples/echo-endpoint (never
# published) is the end-point component it is composed with. Spelling `--workspace` out
# also means the next member added — the ECMA-432 WebSocket binding, say — is checked
# from the day it appears rather than the day someone notices.
#
# --all-features because a feature that is never compiled is a feature that is never
# known to be broken. The `cbor` features act as one — nlipsy-conformance's and
# nlipsy-core's both forward to nlipsy-protocol's, so the flag turns the corpus's
# ECMA-432 §7.1 cases on together with the codec they need rather than leaving them
# silently skipped — and nlipsy-core's `client` / `server` split means half its public
# API is not compiled at all without the flag.
#
# No rustfmt.toml, deliberately. The server has none either, so both are formatted
# at the default width — which is what keeps the code un-reflowed as it moves
# between the two repositories.

# The no-feature `cargo test` line is the other half of the same argument: `cbor` is
# off by default, so every consumer that does not ask for it builds the crate the way
# `--all-features` never exercises — including the `#[cfg]`s that have to keep the
# feature's absence compiling. It is also the run where nlipsy-conformance cannot
# decode an ECMA-432 §7.1 frame, so its CBOR cases are skipped and counted rather than
# reported as passing: 113 passed, 12 skipped there against 125 passed with the feature.
# The corpus's clause-6 exchange cases run in both, because clause 6 is about messages
# and not about wires.
#
# One member is named on a line of its own instead of riding `--all-features`, and the
# reason is a *runtime* one rather than a compile-time preference. `nlipsy-conformance`
# has a `component` feature that runs the same corpus through the built wasm artefact in
# wasmtime (`make check-component`), and `--all-features` would turn it on here — where
# no artefact has been built, so the run would fail for want of a file rather than for
# want of conformance, and every check would first compile a WebAssembly runtime. So the
# `--all-features` lines exclude that member and the line after each names it with
# `--features cbor`, which is `--all-features` minus `component`. The `component` mode's
# own lint and test pass are part of `check-component`, where the artefact exists.
#
# `check-wasm` is deliberately not part of `check`. The crates have to stay wasm-clean
# (a JavaScript package over the same model and the same clause 6 is what comes next,
# and it is why nlipsy-core's traits are bounded by `MaybeSend` rather than `Send`),
# but the target is an extra `rustup target add` that a machine cloning this repository
# may not have, and a check that cannot run is a check that gets commented out. It runs
# `--workspace --all-features`, so every member is covered the day it is added —
# every member but two.
#
# Two members are excluded, and the exclusions are the promise stated precisely rather
# than holes in it. The guarantee is for the message model and the runtime core: those
# are what a JavaScript package compiles, and they must never reach for a socket, a
# clock or a thread. A *native HTTP binding* was never part of it.
#
# nlipsy-http-client: reqwest on wasm32-unknown-unknown is a different client — the
# fetch backend, which has no `connect_timeout` and no `is_connect`, the two things this
# binding's contract is written in terms of — so the crate does not compile there, and
# making it compile would mean shipping a transport that silently keeps neither promise.
# The second transport does not change that and is not an escape from it: hyper over
# tokio is a socket and a reactor, and a browser has neither. What the `hyper` feature
# *is* for is `aarch64-linux-android` and `aarch64-apple-ios`, where there is a socket
# and no room for `jni` or a vendored BoringSSL — `make check-mobile-graph`.
# The JS package will call `fetch` itself and hand the bytes to the core.
#
# nlipsy-server: the same argument from the other end. It is axum over hyper over
# tokio — an accept loop, a thread pool and a socket — and a browser has no listener to
# accept on. A JS package that wants to *serve* NLIP brings its own HTTP framework and
# reaches clause 6 through nlipsy-core, which is exactly why the layer seam and the
# dispatcher live there and not here.
#
# nlipsy-http is *not* excluded, and that is half the reason it is a crate at all:
# ECMA-431 §6 as values — the status map, the header spellings, both directions of
# `Retry-After`, the two error bodies — has no socket and no clock in it
# (`parse_retry_after` takes `now` as an argument precisely so that it has none), and it
# is what a JavaScript `fetch` handler over the same engine needs. The `--workspace`
# line below is what keeps it that way.
#
# nlipsy, the facade, is excluded from the `--all-features` line and checked on its own
# line instead, because `--all-features` on a facade means `full`, and `full` means both
# of the crates above. That would not be a hole in the promise, it would be the promise
# read backwards: a consumer reaching the core from a browser asks for `client-core` or
# `server-core`, and those are exactly the facade features whose crates compile here.
# So the second line names them — every facade feature that is wasm-clean, and no other
# — and it is what fails the day one of them starts pulling a socket in. It cannot reach
# `nlipsy::http`: that module is behind `http-client` or `server`, and naming either
# would pull reqwest or axum onto a target neither compiles for. The crate underneath it
# is covered by the `--workspace` line instead, which is the same coverage by a shorter
# path.

.PHONY: check check-features check-mobile-graph check-wasm check-wit component check-component check-js check-js-e2e serve-example help

help:
	@echo "make check           fmt + clippy + tests, with and without features"
	@echo "make check-features  each feature of the facade and of the client binding"
	@echo "make check-mobile-graph  the hyper transport's graph for Android and iOS"
	@echo "make check-wasm      cargo check for wasm32-unknown-unknown"
	@echo "make check-wit       wasm-tools resolves the interface contract in wit/"
	@echo "make component       builds the wasip3 artefact and nothing else"
	@echo "make check-component builds the component and runs the corpus through it"
	@echo "make check-js        the npm package: build, tsc, its tests, npm pack --dry-run"
	@echo "make check-js-e2e    the three seams jco cannot run yet. Red today, by design."
	@echo "make serve-example   wasmtime serve, on the composed engine + echo end-point"

check: check-features
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --all-features --exclude nlipsy-conformance -- -D warnings
	cargo clippy -p nlipsy-conformance --all-targets --features cbor -- -D warnings
	cargo test --workspace --all-features --exclude nlipsy-conformance
	cargo test -p nlipsy-conformance --features cbor
	cargo test --workspace
# The core with no features at all, named on its own line because `--workspace` stopped
# reaching that build. Feature unification is per-invocation and per-package-graph: the
# no-feature workspace run still compiles nlipsy-core with `client` and `testing` on,
# because nlipsy-http-client's *dev*-dependency asks for them and a `cargo test
# --workspace` builds dev-dependencies. So since the client binding arrived, the one
# build nobody was testing was the plain one — `nlipsy-core = "0.1"`, which is what a
# consumer gets who names the crate and nothing else, and where the `cfg_client!` /
# `cfg_server!` gating has to leave a crate that still compiles.
	cargo test -p nlipsy-core

# Every feature set `--all-features` cannot stand in for: the facade's, one at a time,
# then the client binding's, then the graph the mobile FFI build depends on.
#
# `--all-features` proves nothing about a facade. Every module it has is behind a
# feature, they are nested (`client` implies `http-client` implies `client-core`
# implies `protocol`), and the failure this target exists to catch is a feature that
# forgets part of what it enables: a `http-client` that pulls the binding without
# `nlipsy-core/client` compiles fine under `--all-features`, because something else
# turned that on, and fails for the one consumer who wrote only `http-client`. Each
# line below is a consumer's manifest, spelled out.
#
# `--no-default-features` on every line even though `nlipsy` has none, because the
# thing being checked is what a feature brings and not what a default forgot.
#
# Explicit lines rather than `cargo hack --each-feature`: ten features and three pairs
# are a list a reader can check against the manifest, and cargo-hack is a tool a fresh
# checkout does not have — a check that cannot run is a check that gets commented out,
# which is the same argument that keeps `check-wasm` out of `check`. This one *is* in
# `check`, because it needs no extra toolchain, `cargo tree` included.
check-features:
	cargo check -p nlipsy --no-default-features
	cargo check -p nlipsy --no-default-features --features protocol
	cargo check -p nlipsy --no-default-features --features client-core
	cargo check -p nlipsy --no-default-features --features http-client
	cargo check -p nlipsy --no-default-features --features client
	cargo check -p nlipsy --no-default-features --features server-core
	cargo check -p nlipsy --no-default-features --features server
	cargo check -p nlipsy --no-default-features --features cbor
	cargo check -p nlipsy --no-default-features --features full
# Both roles at once — a deployment that federates. `nlipsy::core` is emitted by either
# `-core` feature, so this is the line that would fail with a duplicate definition if
# the module were written as two `cfg_feature!` blocks instead of one `cfg(any(..))`.
	cargo check -p nlipsy --no-default-features --features client-core,server-core
# Both *bindings* at once, for the same reason one line up: `nlipsy::http` is the second
# module two features produce — ECMA-431 §6 as values is what a client agent and a
# server agent share — and this is the line that fails with a duplicate definition if it
# is ever written as two blocks. The `http-client` and `server` lines above are what
# prove the module appears *at all*: each compiles the facade with one binding and
# nothing else, so a feature that emitted `nlipsy::http` without bringing `nlipsy-http`
# into the build would fail there and nowhere else.
	cargo check -p nlipsy --no-default-features --features http-client,server
# And the *second* transport of the same binding, which is a feature of the facade for
# the same reason the first one is: `http-client` is ECMA-431 §6 over reqwest and
# `http-client-hyper` is ECMA-431 §6 over hyper, neither implies the other, and
# `nlipsy::http_client` is the third module two features produce — so this is the line
# that fails with a duplicate definition if it is ever written as two blocks, and the
# line that fails if a consumer can ask for the hyper transport and not get the module.
	cargo check -p nlipsy --no-default-features --features http-client-hyper
	cargo check -p nlipsy --no-default-features --features http-client,http-client-hyper

# The client binding's own features, below the facade's and for the same argument. The
# crate has two transports now and three builds that `--all-features` cannot stand in
# for: the one with neither (`request`, `FileSink`, `Replay` and the re-exports — what a
# consumer who only replays a recording should have to compile), the one an Android or
# an iOS FFI build actually uses, and the one with both types in it, where a `rustls`
# carrying two crypto providers at once is what `builder_with_provider` exists to
# survive.
	cargo check -p nlipsy-http-client --no-default-features
	cargo check -p nlipsy-http-client --no-default-features --features hyper
	cargo check -p nlipsy-http-client --features hyper
# And run the hyper transport's suite. It is a `test` rather than a `check` because what
# the two transports have to agree on is *behaviour* — the same `TransportError` case for
# the same situation, both `Retry-After` forms, the correlator only when there is one —
# and the two mirrored suites in `src/transport.rs` are where that agreement is written
# down. `cargo test --workspace --all-features` in `check` runs them too; this line is
# what runs them without `cbor`, which is the build a consumer gets.
	cargo test -p nlipsy-http-client --features hyper
# Last, because it is the one gate here that is about a *graph* rather than a build.
	$(MAKE) check-mobile-graph

# The dependency graph the mobile FFI build depends on, as a gate rather than as a note.
#
# `nlipsy-http-client`'s default transport is reqwest 0.13, whose `rustls` feature — and
# its `rustls-no-provider` too — depends on rustls-platform-verifier unconditionally. On
# `aarch64-linux-android` that is `jni` plus a Kotlin companion library the app has to
# ship and initialise; on every target it is aws-lc-sys, a vendored BoringSSL built by
# cmake. The crate's `hyper` feature is the same ECMA-431 §6 exchange with none of it,
# and this target is what keeps it that way — because nothing in the source can say so.
# One transitive default re-enabled three crates down would bring the whole of it back
# and compile perfectly well on a Mac or on a CI runner; the first person to find out
# would be whoever ran `cargo build --target aarch64-linux-android` for the app.
#
# `cargo tree --target` resolves **for a target that is not installed**, with no NDK and
# no Xcode: it reads the lockfile and each manifest's `cfg` predicates and links nothing.
# So this runs anywhere cargo does, Linux CI included, which is why it hangs off
# `check-features` rather than behind a toolchain step. *Compiling* for those two targets
# is the FFI crate's job, on a machine that has the NDK and Xcode; what can be checked
# everywhere is checked everywhere.
#
# `-e normal,build` and not the default: a build-dependency is exactly where the cost
# lives — cmake and cc are build scripts — and dev-dependencies are not in a consumer's
# build at all, so including them would fail this gate on the test harness's own tokio.
MOBILE_TARGETS := aarch64-linux-android aarch64-apple-ios
# Each name is a crate the FFI build cannot carry, and each is a prefix: `jni` catches
# `jni-sys` and the macro crates with it, `aws-lc` catches `aws-lc-rs` and `aws-lc-sys`,
# `rustls-platform-verifier` catches its `-android` half. `cmake` and `security-framework`
# are the C toolchain and the Apple trust store the verifier reaches for; `openssl` and
# `native-tls` are stacks this workspace never had and must not acquire by accident.
MOBILE_FORBIDDEN := jni rustls-platform-verifier aws-lc cmake security-framework openssl native-tls
# And the three that must be there, printed rather than merely checked: a gate that only
# says "none of the bad ones" would pass just as happily on a graph with no TLS in it.
MOBILE_REQUIRED := ring webpki-roots cc

check-mobile-graph:
	@for target in $(MOBILE_TARGETS); do \
	  echo "== $$target: nlipsy-http-client --no-default-features --features hyper"; \
	  tree=$$(cargo tree -p nlipsy-http-client --no-default-features --features hyper \
	          -e normal,build --prefix none --target $$target) || exit 1; \
	  for crate in $(MOBILE_FORBIDDEN); do \
	    if echo "$$tree" | grep -qi "^$$crate"; then \
	      echo "   FAIL: $$crate is in the graph for $$target:"; \
	      echo "$$tree" | grep -i "^$$crate" | sort -u | sed 's/^/     /'; \
	      exit 1; \
	    fi; \
	  done; \
	  for crate in $(MOBILE_REQUIRED); do \
	    line=$$(echo "$$tree" | grep -i "^$$crate v" | sort -u | head -1); \
	    if [ -z "$$line" ]; then \
	      echo "   FAIL: $$crate is missing from the graph for $$target"; \
	      exit 1; \
	    fi; \
	    echo "   TLS: $$line"; \
	  done; \
	  echo "   $$(echo "$$tree" | grep -v '(\*)$$' | sort -u | wc -l | tr -d ' ') crates, and none of: $(MOBILE_FORBIDDEN)"; \
	done

# A third and a fourth exclusion joined the two above, and neither is a hole in the
# promise either: `nlipsy-component` and `examples/echo-endpoint` are `cdylib`s for
# `wasm32-wasip3`, the target `wit_bindgen::generate!` writes canonical-ABI entry points
# for. On `wasm32-unknown-unknown` there is no component model and no WASI, so the
# artefact would be a bare core module with dangling imports — a build that says nothing
# about anything. `make check-component` is where both are checked, on the target they
# have.
check-wasm:
	cargo check --workspace --all-features --exclude nlipsy-http-client --exclude nlipsy-server --exclude nlipsy --exclude nlipsy-component --exclude echo-endpoint --exclude nlipsy-conformance --target wasm32-unknown-unknown
	cargo check -p nlipsy-conformance --features cbor --target wasm32-unknown-unknown
	cargo check -p nlipsy --no-default-features --features client-core,server-core,cbor --target wasm32-unknown-unknown

# `wit/nlipsy.wit` is the *interface* contract — what the component of the JavaScript
# package offers and what it asks of its host — the way `conformance/cases/` is the
# behaviour contract. This target resolves it: every `use` followed, every type
# reachable, the world well-formed. `wit/README.md` §2 is the whole of how the contract
# is validated; this is the first and cheapest of the checks there, and the only one
# that needs no wasm target at all.
#
# Not part of `check`, for exactly the reason `check-wasm` is not: it needs a tool a
# fresh checkout does not have (`cargo install wasm-tools --locked`), and a check that
# cannot run is a check that gets commented out. `check` must stay runnable with cargo
# and nothing else. CI is where the tool is installed once and this runs every time:
# the workflow that builds the component needs wasm-tools anyway.
check-wit:
	wasm-tools component wit wit/ > /dev/null
# And the seed rule, which `wit/README.md` §1 states and nothing used to enforce: every
# citation in the contract and in that README — a path, `::`, an item — must name an
# item the file defines (`wit/README.md` §2 has the exact rule). An item rather than a
# line, because a line number drifts the moment the file above it grows, and a rule that
# can be wrong without anything noticing is a convention. Node and no dependency, so it
# belongs beside the line above rather than in a target of its own.
	node wit/check-seeds.mjs

# The component, and the corpus through it.
#
# `crates/nlipsy-component` is the `wit-bindgen` guest of the contract above: the same
# engine, re-spelled in the contract's vocabulary and built for `wasm32-wasip3`, where
# `cargo build` emits a *component* with no `cargo-component` and no adapter of any kind,
# and where `std` links WASI **0.3** — `wasi:cli@0.3.0` and `wasi:clocks@0.3.0`, no
# `wasi:io`, because 0.3 carries streams natively.
# Four steps for the artefact, then the corpus and the composition:
#
#   1. build with the `component` profile (root `Cargo.toml`; Cargo reads `[profile.*]`
#      only there, which is why it is not in the member's manifest);
#   2. `wasm-tools validate` with every proposal on, because the async canonical ABI is
#      one of them and a default validation would reject the artefact rather than check
#      it;
#   3. `wasm-tools component wit`, which is also the check that would catch a regression
#      to a non-component core module — and the list it prints is what says the imports
#      are 0.3 and nothing else;
#   4. the 125 cases of `conformance/` through the artefact in wasmtime, plus the async
#      smoke and the re-entry proof. The host links them with `wasmtime_wasi::p3`.
#
# Not part of `check`, for the reason `check-wasm` and `check-wit` are not, four times
# over: it needs a second toolchain, an extra `rustup target add` on it, a tool a fresh
# checkout does not have (`cargo install wasm-tools --locked`), and a WebAssembly runtime
# whose compile time is minutes. `check` must stay runnable with cargo and nothing else.

# The toolchain the *artefact* is built with, and the one thing in this file that is
# dated. `rust-std` for `wasm32-wasip3` ships on nightly today and on stable at **Rust
# 1.100, 2026-11-12**; on that day this becomes `stable` and the variable can go. It is a
# variable rather than a literal `+nightly` so that the date is written once, so that a
# contributor whose nightly is a different day can override it on the command line
# (`make check-component COMPONENT_TOOLCHAIN=nightly`), and so that CI pins the same
# string the Makefile does.
#
# It is deliberately *not* `nlipsy-component`'s `rust-version`: that is what the crate's
# source needs (1.89, for wit-bindgen 0.62 and edition 2024), and the crate's `rlib` half
# is compiled and tested by `make check` on stable. Raising it to the target's floor
# would stop that and prove nothing.
COMPONENT_TOOLCHAIN ?= nightly-2026-09-15
COMPONENT := target/wasm32-wasip3/component/nlipsy_component.wasm
# The deployment half: `examples/echo-endpoint` exports the five interfaces the engine
# imports, and `wac plug` joins them. What comes out imports only `wasi:*` — it is a
# complete server — and is what `wasmtime serve` serves and `make serve-example` runs.
ENDPOINT := target/wasm32-wasip3/component/echo_endpoint.wasm
COMPOSED := target/wasm32-wasip3/component/nlipsy_echo.wasm

# The artefact on its own, for a consumer that only wants the `.wasm`.
#
# `javascript/scripts/build.mjs` runs this when `NLIPSY_COMPONENT` names nothing and
# nothing is at $(COMPONENT), so that the npm package builds the component with the same
# toolchain and the same profile as everything else rather than with a second copy of the
# command. `check-component` calls it too, in the same position its build line was in, so
# that the order of that target is exactly what it was.
component:
	cargo +$(COMPONENT_TOOLCHAIN) build -p nlipsy-component --profile component --target wasm32-wasip3

check-component:
	cargo clippy -p nlipsy-conformance --all-targets --features cbor,component -- -D warnings
	$(MAKE) component
	wasm-tools validate --features all $(COMPONENT)
	wasm-tools component wit $(COMPONENT) > /dev/null
# The example, and the composition. `wac` is a fifth thing a fresh checkout does not
# have (`cargo install wac-cli --locked`), which is why this whole target is out of
# `check` and not why one more line is: the same argument, one tool further along.
	cargo +$(COMPONENT_TOOLCHAIN) build -p echo-endpoint --profile component --target wasm32-wasip3
	wasm-tools validate --features all $(ENDPOINT)
	wac plug $(COMPONENT) --plug $(ENDPOINT) -o $(COMPOSED)
	wasm-tools validate --features all $(COMPOSED)
	wasm-tools component wit $(COMPOSED) > /dev/null
	cargo test -p nlipsy-conformance --features cbor,component

# The npm package `nlipsy`: a loader and five adapters over the artefact above.
#
# It is **on**: measured against jco 1.39.0 on 2026-10-21, every export of this component
# and all five of its host imports work, and the whole 125-case corpus passes through the
# package in Node and in Bun. What does not work is three seams — `wasi:http/handler`,
# `wasi:http/client`, and a host callback suspended across a turn of the event loop — and
# those are `check-js-e2e` below rather than an excuse to gate nothing.
#
# `npm ci` and not `npm install`: the lockfile is the pinned jco and the pinned shim, and
# a gate that resolved a fresh minor would be measuring something else. Five lines rather
# than one `npm run` script, so that a failure names the step it was in.
#
# Not part of `check`, for the reason `check-wasm`, `check-wit` and `check-component` are
# not: it needs Node and a network for `npm ci`, and the artefact, which needs the second
# toolchain. `check` must stay runnable with cargo and nothing else.
check-js:
	cd javascript && npm ci
	cd javascript && COMPONENT_TOOLCHAIN=$(COMPONENT_TOOLCHAIN) npm run build
	cd javascript && npm run typecheck
	cd javascript && npm test
	cd javascript && npm pack --dry-run

# The three seams jco 1.39.0 cannot run, unskipped.
#
# **This target is red today and is meant to be**: it is how the day jco fixes the defect
# is noticed, and how a reader checks that the failures are still the known ones rather
# than something of ours. `javascript/test/e2e/reason.mjs` holds the three traps verbatim
# and `javascript/README.md` what each blocks. Not in CI, for the same reason.
check-js-e2e:
	cd javascript && npm run test:e2e

# The composed artefact, served over a socket, for looking at by hand.
#
# It is the whole deployment story in one line: no JavaScript, no host language, no
# framework — one `.wasm` and a runtime that speaks WASI 0.3. `make check-component`
# already runs a request through the same artefact in-process, so this is for a human
# with `curl` rather than for CI.
#
#   curl -s -X POST http://127.0.0.1:8080/nlip \
#        -H 'content-type: application/json' \
#        -d '{"format":"text","subformat":"english","content":"What is Ecma?"}'
SERVE_ADDR ?= 127.0.0.1:8080

serve-example: $(COMPOSED)
	wasmtime serve --addr $(SERVE_ADDR) $(COMPOSED)
