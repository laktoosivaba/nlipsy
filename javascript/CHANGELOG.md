# Changelog

All notable changes to the npm package `nlipsy`. The format is
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versioning is
[semantic](https://semver.org/spec/v2.0.0.html).

The package's version is its own. What it wraps is a *component*, and the three things
that decide what this package can do all have versions of their own: the interface
contract `wit/nlipsy.wit`, the crates the component is built from, and jco. Every entry
below names all three.

## [Unreleased]

A loader and five adapters over a component that carries the protocol, the composition
and the ECMA-431 §6 binding itself (`wit/README.md`, *The design*).

**Not published.** Three seams of jco 1.39.0 do not run yet (README, "What runs today,
and what does not"); a package whose `serve()` starts a listener that cannot answer is
not one to install.

- Contract: `nlipsy:engine@0.1.0` (`wit/nlipsy.wit`).
- Crates the component is built from: `nlipsy-protocol` 0.1.2, `nlipsy-core` 0.1.2,
  `nlipsy-http` 0.1.2, through `nlipsy-component` 0.1.2 (`publish = false`).
- Toolchain: Rust `nightly-2026-09-15` for `wasm32-wasip3`, wit-bindgen 0.62.0.
- `@bytecodealliance/jco` 1.39.0, `@bytecodealliance/preview3-shim` 0.11.0.

### Added

- `createEndpoint({ answer, layers, authority, config, rejection, http })` — installs one
  deployment and resolves its configuration against the engine's own defaults;
  `endpoint.serve(bytes, wire)` runs the composed order over one frame.
- `serve({ port, ...endpoint })` — the same end-point on a socket, over the component's
  `wasi:http/handler@0.3.0` through `@bytecodealliance/preview3-shim`'s `HttpServer`. No
  HTTP code of ours.
- `createClient({ url | transport | replayFrom, wire, recordTo })` — the contract's four
  client functions, one per set of options, with the replay cursor kept here.
- `pass()` / `answer()` — the two halves of a layer's way in. A layer is a
  `{ before, after }` pair the *engine* composes.
- The clause-5 functions (`decode`, `encode`, `validate`, `isControl`, `extractText`,
  `firstSubmessageToken`, `tokenCount`, `tokenKindOf`, `uploadRequest`, `uploadResponse`),
  the engine's defaults (`defaultConfig`, `defaultRejection`, `defaultHttpOptions`) and
  its four rules (`owedBy`, `publicMessage`, `retryAfterOf`, `errorReply`).
- `corpus.check`, `corpus.finish`, `corpus.serveMessage` — clause 6's steps on their own,
  for a conformance runner and for nothing else.
- Typed failures a handler throws: `unsupported`, `unavailable`, `refused`, `timedOut`;
  and for a transport, `unreachableError`, `interruptedError`, `malformedTransportError`.
  A plain `Error` thrown by a handler becomes `endpoint-error.internal` and stays on this
  side as the `cause` of what is thrown.
- `Reply.tokens` and `Reply.authenticationTokens` beside `Reply.conversationTokens` —
  §6.2 ¶2's obligation and §6.2.2's credential, both derived by the engine.
- `Context` as a plain object with an `exchange: bigint` a host keys its own state on.
- ESM only, one entry point: the component is instantiated by a top-level `await`, and
  everything that reaches the engine is `async`, `decode` and `encode` included, because
  jco's output for a wasip3 component returns a promise from every export.
