/**
 * A host callback that waits for something real.
 *
 * `host-endpoint.answer`, `host-layers.before`/`.after`, `host-transport.send` and
 * `host-record-sink.write` are `async func` in the contract for one reason: *because it
 * is the user's code and the user's code waits for things*. Today a callback that waits
 * for anything beyond the microtask queue traps the component, so the handler that
 * queries a database — which is every real handler — is exactly what does not run yet.
 * These are the tests that will say when it does.
 */
import { setTimeout as delay } from 'node:timers/promises';
import { describe, it, expect } from 'vitest';

import { api } from '../spec/index.mjs';
import { BLOCKERS, RUN, announce } from './reason.mjs';

announce('a host callback that awaits the event loop', BLOCKERS.eventLoop);

const REQUEST = { format: 'text', subformat: 'english', content: 'What is Ecma?' };

describe.skipIf(!RUN)('a host callback that awaits the event loop', () => {
  it('a handler that awaits a timer still answers', async () => {
    const endpoint = await api.createEndpoint({
      answer: async (request) => {
        await delay(10);
        return { ...request, content: 'answered after waiting' };
      },
    });
    try {
      const served = await endpoint.serve(await api.encode(REQUEST, 'json'), 'json');
      expect(served.outcome.kind).toBe('ok');
      expect(served.reply.content).toBe('answered after waiting');
    } finally {
      endpoint.close();
    }
  });

  it('a layer that awaits a timer in both halves still unwinds in order', async () => {
    const seen = [];
    const endpoint = await api.createEndpoint({
      answer: async (request) => {
        await delay(5);
        seen.push('answer');
        return request;
      },
      layers: [
        {
          name: 'slow',
          before: async (request) => {
            await delay(5);
            seen.push('before');
            return api.pass(request);
          },
          after: async (reply) => {
            await delay(5);
            seen.push('after');
            return reply;
          },
        },
      ],
    });
    try {
      const served = await endpoint.serve(await api.encode(REQUEST, 'json'), 'json');
      expect(served.outcome.kind).toBe('ok');
      expect(seen).toEqual(['before', 'answer', 'after']);
    } finally {
      endpoint.close();
    }
  });

  it('a transport over `fetch` carries an exchange — the browser’s route', async () => {
    // Not a fetch against a socket, which would be a second thing to go wrong: what is
    // being tested is that a transport doing real I/O can answer at all.
    const client = api.createClient({
      transport: {
        send: async (bytes, cx) => {
          await delay(5);
          expect(cx.exchange).toBeGreaterThan(2n ** 32n);
          return {
            body: new TextEncoder().encode(
              `{"format":"text","subformat":"english","content":"carried ${bytes.length} bytes"}`,
            ),
            detail: { kind: 'http', status: 200 },
          };
        },
      },
    });
    const reply = await client.call(REQUEST);
    expect(reply.text).toMatch(/^carried \d+ bytes$/);
  });

  it('two clients whose transports genuinely overlap are still told apart', async () => {
    // The keying is exact by construction — each `call-through` carries the number this
    // package minted — but today jco serialises the two exported calls, so the overlap
    // this asserts cannot happen yet. `test/spec/host.mjs` asserts the keying without it.
    const entered = [];
    const client = (label, wait) =>
      api.createClient({
        transport: {
          send: async (_bytes, cx) => {
            entered.push(`enter ${label}@${cx.exchange}`);
            await delay(wait);
            entered.push(`leave ${label}`);
            return {
              body: new TextEncoder().encode(
                `{"format":"text","subformat":"english","content":"${label}"}`,
              ),
              detail: { kind: 'http', status: 200 },
            };
          },
        },
      });
    const [first, second] = await Promise.all([
      client('one', 30).call(REQUEST),
      client('two', 1).call(REQUEST),
    ]);
    expect(first.text).toBe('one');
    expect(second.text).toBe('two');
    expect(entered.slice(0, 2)).toEqual([entered[0], expect.stringMatching(/^enter two@/)]);
  });
});
