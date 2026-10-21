# nlipsy

NLIP — [ECMA-430][ecma-430] — as a WebAssembly component, and the small amount of
JavaScript that gives it your code.

**The protocol is not in this package.** The message model, clause 5's rules, clause 6's
exchange and the order it runs in, the composition of your layers over your handler, and
[ECMA-431][ecma-431] §6's HTTP binding in both directions are all inside
`nlipsy_component.wasm`, which is the same Rust engine `nlipsy-core` and `nlipsy-http`
are. What is here is a **loader**, **five adapters** from your objects to the component's
five imports, the conversion of values across the boundary, and two pieces of state the
contract deliberately does not keep — a client's URL and a replay cursor.

That is the whole design, and it is the point of it: a second host language is measured
against the same two contracts as the first — `conformance/cases/` for behaviour,
`wit/nlipsy.wit` for the interface — and writes the same small amount of code. If the
wrapper ever seems to need a rule, a status code, a header name or a decision about which
layer runs first, the contract is what is missing.

```js
import { serve, createClient, pass } from 'nlipsy';

const server = await serve({
  port: 8080,
  config: { issueConversationToken: true },
  layers: [{ name: 'audit', after: (reply, cx) => (console.log(cx.exchange), reply) }],
  answer: (request) => ({ ...request, content: `you said: ${request.content}` }),
});
```

> **`serve()` is the shape, and it does not answer a request yet.** Under jco 1.39.0 the
> listener starts and the first request traps — *What runs today, and what does not*,
> immediately below, has the measurement. Everything *else* in that example runs today:
> the same options through `createEndpoint({ … })`, driven with
> `endpoint.serve(bytes)`, is the end-to-end path that works, and so is
> `createClient({ transport })`.

There is no status code in that, no header, no path, no `Retry-After` and no order for the
layer to run in. Every one of those is in the component.

---

## What runs today, and what does not

The package is **not published**. It will be, the day the three seams below run: a
package whose `serve()` starts a listener that cannot answer a request is not one anybody
should install.

Measured on **2026-10-21** against `@bytecodealliance/jco` **1.39.0** and
`@bytecodealliance/preview3-shim` **0.11.0** — both the latest published versions — on
Node 26.5.0 and Bun 1.4.0.

### Runs

- **Every export of the component**: the clause-5 functions, clause 6's `check` and
  `finish`, the composed order over bytes and over a message, the engine's defaults and
  its four rules, `client.call-through` and `client.replay`.
- **All five host imports**: your handler, your layers with both halves, your token
  authority, your transport, your record sink — as long as a callback settles inside the
  microtask queue (see below).
- **The whole conformance corpus**, 125 cases, through this package, in **Node and Bun**:
  `npm test` runs it in Node and spawns Bun to run it again.
- **A client over a transport you supply** — `createClient({ transport })` — which is the
  browser's route and is also how a test drives a client with no socket anywhere.

### Does not run yet

Three seams, all of them jco's and none of them avoidable by any design of ours:

| what | how it fails |
|---|---|
| `serve()` — the component's `wasi:http/handler` on a socket | the listener starts; the first request traps the moment the guest reads its path, `RuntimeError: wasm trap: out of bounds memory access`, and every call into the instance after that is refused with `wasm trap: cannot enter component instance` |
| `createClient({ url })` and `{ url, recordTo }` — `wasi:http/client` | `RuntimeError: memory access out of bounds` |
| **a host callback that awaits the *event loop*** — a timer, a socket, a `fetch` | `RuntimeError: unreachable`, then `cannot access a Thread Local Storage value during or after destruction: AccessError` on the calls after it |

The third is the one to read twice: a handler that awaits a microtask is fine and a
handler that awaits a database is not, which means *the real handler is the one that does
not run yet*. The suites for all three are written, are skipped, and print that reason —
`test/e2e/`, `npm run test:e2e` or `make check-js-e2e` runs them unskipped and shows the
traps. Under wasmtime 48 every one of these paths works today; `make serve-example` in the
workspace above serves the same engine over `curl` with no JavaScript in it at all.

### Will not run, for reasons that are not defects

- **Bun cannot serve.** `HttpServer.listen` moves a `ReadableStream` and a `MessagePort`
  into a `node:worker_threads` worker and Bun refuses the transfer list
  (`DOMException [DataCloneError]`). Bun runs everything else, the corpus included.
- **A browser is a client and nothing else.** A page cannot serve an incoming
  `wasi:http/handler` at all. The preview3 shim's browser build does carry an outgoing
  `wasi:http/client` over the preview2 shim's `fetch`, but that is the seam that traps
  under jco in Node and it is not measured here. The browser's route is
  `createClient({ transport })` over `fetch`, which is `examples/browser-client/`.
- **Firefox and Safari** wait for JSPI. Node 26, Bun 1.4 and Chromium 137+ have it.

---

## Install

```sh
npm install nlipsy     # when it is published; see above
```

ESM only, and that is not a preference: the component is instantiated by a top-level
`await` in jco's glue, and CommonJS has none. Node 26+, Bun 1.4+, Chromium 137+.

---

## The API

Everything that reaches the engine is **`async`**, `decode` and `encode` included. That is
not a design choice either — jco's output for this component returns a `Promise` from every
export, including the ones WIT declares as a plain `func` (see
[What jco actually renders](#what-jco-actually-renders)).

### An end-point

```js
import { createEndpoint, pass, answer, unavailable } from 'nlipsy';

const endpoint = await createEndpoint({
  name: 'echo',

  // The clause-6 switches. What they *mean* is clause 6's, and clause 6 is inside.
  config: { issueConversationToken: true },

  // Outermost first. The engine composes them: outermost first on the way in, innermost
  // first on the way out, with clause 6's finishing step underneath all of them.
  layers: [
    {
      name: 'cache',
      before(request, cx) {
        const hit = cache.get(String(request.content));
        return hit === undefined ? pass(request) : answer(hit);   // short-circuits
      },
      after(reply, cx) {
        return reply;                                             // after `finish`
      },
    },
  ],

  // §6.2's exception, §6.2.1's confirmation, the minting. All three optional; all three
  // synchronous, because they run between the handler and the wire.
  authority: {
    isOwn: (token, cx) => sessions.owns(token),
    issued: (kind, cx) => sessions.issued(cx.exchange, kind),
    mint: (kind, cx) => sessions.mint(cx.exchange, kind),
  },

  // Your handler. It returns a *message*: finishing is the engine's.
  answer(request, cx) {
    if (busy) throw unavailable('the queue is full', 5_000);
    return { format: 'text', subformat: 'english', content: 'hello' };
  },
});

// One exchange over the bytes of one frame — for a transport that is not HTTP.
const served = await endpoint.serve(bytes, 'json');
served.outcome;   // { kind: 'ok' } | { kind: 'refused', … } | { kind: 'failed', … }
served.reply;     // a message on every outcome, refusals included

endpoint.close();
```

`serve({ port, ...endpoint })` is the same end-point on a socket, over the component's
own `wasi:http/handler`. Everything a deployment has to decide about that binding — the
body limit, the correlator header, the CORS rule, the nine status codes — is
`Endpoint.http`, read by the guest; `defaultHttpOptions()` is the reference server's
answer to all of it.

**One process serves one end-point.** The component asks its host for one and hands no key
with the question — `host-endpoint.name()`, `.conformity()` and `host-layers.count()` take
no arguments, and `wit/nlipsy.wit` says why: *"There is one end-point per component
instance: a deployment that answers two kinds of request routes inside `answer`."* A
second `createEndpoint` is refused rather than allowed to take turns with the first. Run a
second end-point in a second process or worker, or route inside your handler —
`isControl(request)` is exported for exactly that.

### A client

```js
import { createClient } from 'nlipsy';

const client = createClient({ url: 'https://example.com/nlip' });
const reply = await client.call({ format: 'text', subformat: 'english', content: 'hi' });

reply.text;                  // every text part, in document order
reply.tokens;                // what §6.2 ¶2 obliges you to send back next time
reply.conversationTokens;    // §6.2.1's identifier, as a key
reply.authenticationTokens;  // §6.2.2's credential, as a key
reply.isError;
reply.detail.status;
reply.detail.retryAfterMs;   // 0 and undefined are different answers
```

Four routes, one per set of options, and the combinations that name none are refused at
construction rather than resolved by precedence:

| given | route | carried by |
|---|---|---|
| `url` | `client.call` | `wasi:http/client`, inside the component |
| `transport` | `client.call-through` | your object, through `host-transport` |
| `url` + `recordTo` | `client.call-recording` | as `call`, each exchange written as one JSON line |
| `replayFrom` | `client.replay` | a recording, cursor kept here |

A recording written by `recordTo` is the one `replayFrom` reads and the one a Rust test
loads: the spelling is `Recorded::to_json_line`'s, written inside the component, and
nothing in this package parses or produces it.

**One process installs one record sink.** `host-record-sink.write(line)` carries no
context, so two recordings could not be told apart; a second recording client is refused
rather than mixed into the first.

### The message model

```js
import { decode, encode, validate, isControl, extractText, tokenKindOf } from 'nlipsy';
```

`decode`, `encode`, `validate`, `isControl`, `extractText`, `firstSubmessageToken`,
`tokenCount`, `tokenKindOf`, `uploadRequest`, `uploadResponse` — each one call into the
component. `tokenKindOf` is a function rather than something you write because it is a
*rule* (§5.3 read through §6.2); which submessages are tokens at all is not, and is
`format === 'token'`.

A message is a plain object and `content` is a plain value: a string, a number, a
`boolean`, `null`, a `Uint8Array` for a byte string, an array or an object for structured
content — and a **`bigint`** for a JSON integer wider than a `number` holds, at the top
level *and nested*, because rounding a peer's 64-bit identifier is losing their data.

### Errors

Peer input cannot trap the component: `decode` returns a result and `serve` returns a
`served` with an `outcome`. At the JavaScript seam jco turns a result's error into a
throw, so this package converts each one into an error carrying the contract's value:
`MalformedError.error`, `RefusedError.rejected`, `TransportFailedError.error`,
`ReplayError`.

The other direction is the one that matters more. A plain `Error` thrown by your handler
would **escape the lowering** and take the whole call with it, so every one of the ten
host callbacks is wrapped: a thrown `Error` becomes `endpoint-error.internal` with its
message, and `unsupported()`, `unavailable()`, `refused()` and `timedOut()` build the
other four cases when your end-point is making a decision rather than hitting a bug.

Three of the ten have no error channel at all — the token authority's answers are
synchronous `func`s returning plain values — so an exception there is reported through
`onHostError` and the safe default is answered (`false`, `undefined`). The same holds for
the four configuration reads and the layer count.

---

## What jco actually renders

`wit/README.md` §5 is the table of what jco renders for this world, and this is what jco
1.39.0 emits. The type-level rules in that table all hold; three things are worth
knowing beside it.

- **Every export returns a `Promise`.** The generated `.d.ts` types a WIT `func` as
  returning `T` — `decode(bytes, wire): Message` — and at run time it returns
  `Promise<Message>`. Every call in this package is awaited; the public API is async
  throughout. §5's table says `func → export function name(…): T`, which is the
  declaration and not the value.
- **Doc comments are not carried.** wit-bindgen copies the contract's `///` into the
  generated Rust; jco 1.39.0 emits a `/** @module … **/` banner per interface and a
  `# Variants` block for each `enum`, and nothing per function, record or field. A
  JavaScript caller's editor gets its documentation from **this package's** types, which
  is why `src/types.ts` carries the clause citations rather than pointing at the
  contract.
- **The five `--map` targets resolve relative to the output directory**, and the interface
  is named **without** its version: `nlipsy:engine/host-endpoint`, not
  `…@0.1.0`. Both are in `scripts/build.mjs`.

The three traps §6 does name are all real: `Number` is shadowed in
`interfaces/nlipsy-engine-types.d.ts`, `context.exchange` is a `bigint` whose `0n` is
falsy and meaningful, and every `result` is a thrown `ComponentError` on the export side
and a `catch`-and-convert on the import side.

---

## Building it

The package is built from the workspace, not from a tarball:

```sh
make check-js          # the component, jco, tsc, the tests, `npm pack --dry-run`
make check-js-e2e      # the three blocked seams, unskipped. Fails today, by design.
```

or, inside `javascript/`:

```sh
npm ci
npm run build          # cargo → jco transpile → tsc → dist/
npm run typecheck      # the wrapper and the examples
npm test               # the glue's suites, the corpus, Bun; the e2e suites skip loudly
```

`npm run build` builds the component with the pinned nightly the `Makefile` names
(`COMPONENT_TOOLCHAIN`, `nightly-2026-09-15` until `wasm32-wasip3` is stable on Rust
1.100, 2026-11-12) unless `NLIPSY_COMPONENT` already points at an artefact.

`@bytecodealliance/jco` is pinned to **1.39.0** and `@bytecodealliance/preview3-shim` to
**0.11.0**. Both are exact rather than caret-ranged: they are the two things this package's
correctness is measured against, and a floating minor would change what "runs today"
means without anybody deciding to.

## Licence

MIT or Apache-2.0, at your option.

[ecma-430]: https://ecma-international.org/publications-and-standards/standards/ecma-430/
[ecma-431]: https://ecma-international.org/publications-and-standards/standards/ecma-431/
