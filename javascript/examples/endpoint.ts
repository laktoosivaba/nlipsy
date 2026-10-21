/**
 * An end-point on a socket, with one layer and a conversation token.
 *
 * ```sh
 * npm run build            # the component, jco, tsc
 * node --experimental-strip-types examples/endpoint.ts
 * curl -sS -i -X POST http://127.0.0.1:8080/nlip \
 *   -H 'content-type: application/json' \
 *   -d '{"format":"text","subformat":"english","content":"What is Ecma?"}'
 * ```
 *
 * **It does not answer yet**, and the reason is not this file's: jco 1.39.0 traps the
 * moment the component reads the request's path through the shim's `wasi:http/types`
 * (README, *What runs today, and what does not*). The listener starts. The same
 * end-point run under `wasmtime serve` answers today — that is `make serve-example` in
 * the workspace above, over a Rust end-point composed with the same engine.
 *
 * What is worth reading here is how little of it there is. There is no status code in
 * this file, no header, no path, no `Retry-After`, no order for the layer to run in, no
 * decision about which token to echo. Every one of those is inside the component.
 */

import { pass, serve, unavailable } from 'nlipsy';
import type { Context, Layer, Message } from 'nlipsy';

/** One conversation, remembered for as long as the process is. */
const conversations = new Map<bigint, string>();

/**
 * A layer that turns a control message away during maintenance.
 *
 * Its `before` either passes the request down or answers in the handler's place; its
 * `after` sees the reply that goes on the wire, **after** clause 6 has finished it, so
 * the token minted below is already on the message it logs.
 */
const maintenance: Layer = {
  name: 'maintenance',
  before(request: Message) {
    if (process.env.MAINTENANCE === '1') {
      throw unavailable('this end-point is being upgraded', 30_000);
    }
    return pass(request);
  },
  after(reply: Message, cx: Context) {
    console.log(`[${cx.correlator ?? '-'}] exchange ${cx.exchange} answered`);
    return reply;
  },
};

const server = await serve({
  name: 'echo',
  port: Number(process.env.PORT ?? 8080),

  // §6.2.1's optional half, turned on. What it *means* — when to mint, when to confirm,
  // what a refusal still owes — is clause 6's, and clause 6 is in the component.
  config: { issueConversationToken: true },

  layers: [maintenance],

  authority: {
    isOwn: (token) => [...conversations.values()].includes(token),
    issued: (kind, cx) => (kind === 'conversation' ? conversations.get(cx.exchange) : undefined),
    mint: (kind, cx) => {
      if (kind !== 'conversation') return undefined;
      const minted = `echo_${cx.exchange}`;
      conversations.set(cx.exchange, minted);
      return minted;
    },
  },

  answer(request: Message) {
    return { ...request, content: `you said: ${String(request.content)}` };
  },
});

console.log(`listening on http://${server.host}:${server.port}/nlip`);
process.on('SIGINT', () => {
  void server.close().then(() => process.exit(0));
});
