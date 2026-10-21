/**
 * The five host modules: their defaults, their error conversion, and the registry that
 * says whose objects a callback belongs to.
 *
 * Every assertion here is about the *wrapper*. What the engine does with what these
 * modules answer — the order the layers run in, what a short-circuited reply still owes,
 * which token is echoed — is the corpus's, and is asserted in `corpus.mjs` through the
 * same component.
 */

import { deepEqual, equal, ok, throws } from './assert.mjs';

const REQUEST = { format: 'text', subformat: 'english', content: 'What is Ecma?' };
const WITH_TOKEN = {
  ...REQUEST,
  submessages: [{ format: 'token', subformat: 'conversation', content: 'peer-1' }],
};

/** Install an end-point, run `body` against it, and always release the slot. */
async function withEndpoint(api, options, body) {
  const endpoint = await api.createEndpoint(options);
  try {
    return await body(endpoint);
  } finally {
    endpoint.close();
  }
}

/** One exchange over bytes, as `engine.serve` runs it. */
async function serve(api, endpoint, message = REQUEST) {
  return await endpoint.serve(await api.encode(message, 'json'), 'json');
}

export const specs = [
  {
    name: 'a deployment that supplies only a handler gets every default',
    async run(api) {
      const defaults = await api.defaultConfig();
      await withEndpoint(api, { answer: (request) => request }, async (endpoint) => {
        // WIT has no optional imports, so the wrapper supplies
        // what a Rust deployment gets from the trait's default methods — zero layers, an
        // authority that owns nothing and mints nothing.
        deepEqual(endpoint.config, defaults, 'the clause-6 switches are the engine’s');
        equal(endpoint.name, 'nlipsy', 'a nameless end-point still has a name for the log');

        const served = await serve(api, endpoint, WITH_TOKEN);
        equal(served.outcome.kind, 'ok', 'the exchange did not complete');
        // The peer's token comes back because nothing owns it — which is what an
        // authority with no `isOwn` means. The rule is the engine's; what is asserted
        // here is that the default answered `false` rather than throwing.
        const tokens = (served.reply.submessages ?? []).filter((one) => one.format === 'token');
        equal(tokens.length, 1, `expected the peer's token echoed, got ${tokens.length}`);
        equal(tokens[0].content, 'peer-1', 'the echoed token is not the one that arrived');
      });
    },
  },

  {
    name: 'a handler that throws becomes `internal`, and nothing escapes the lowering',
    async run(api) {
      await withEndpoint(
        api,
        {
          answer: () => {
            throw new Error('kaboom');
          },
        },
        async (endpoint) => {
          const served = await serve(api, endpoint);
          equal(served.outcome.kind, 'failed', 'a thrown Error did not fail the exchange');
          equal(served.outcome.error.kind, 'internal', 'it was not `internal`');
          ok(
            served.outcome.error.detail.includes('kaboom'),
            `the detail should carry the message: ${served.outcome.error.detail}`,
          );
          // And the peer still gets a message, because `serve` never fails: the detail
          // of an `internal` fault is for the log (ECMA-434 §7.3.3) and the engine is
          // what decides what the peer sees.
          equal(served.reply.messageType, 'Error', 'the peer was not answered in NLIP');
          ok(
            !JSON.stringify(served.reply).includes('kaboom'),
            'the internal detail reached the peer',
          );
        },
      );
    },
  },

  {
    name: 'a handler that throws an EndpointFailure keeps the case it chose',
    async run(api) {
      await withEndpoint(
        api,
        {
          answer: () => {
            throw api.unavailable('the model is warming up', 2_000);
          },
        },
        async (endpoint) => {
          const served = await serve(api, endpoint);
          equal(served.outcome.kind, 'failed', 'the exchange did not fail');
          equal(served.outcome.error.kind, 'unavailable', 'the chosen case was lost');
          equal(served.outcome.error.retryAfterMs, 2_000, 'the delay was lost');
          // The rule that turns a delay into `Retry-After` is the engine's, and this is
          // how a host asks it rather than restating it.
          equal(await api.retryAfterOf(served.outcome), 2_000, 'the engine read it back');
        },
      );
    },
  },

  {
    name: 'a layer whose `before` throws becomes `internal`, naming the layer',
    async run(api) {
      await withEndpoint(
        api,
        {
          answer: () => {
            throw new Error('the handler must not be reached');
          },
          layers: [
            {
              name: 'auth',
              before: () => {
                throw new Error('no credentials');
              },
            },
          ],
        },
        async (endpoint) => {
          const served = await serve(api, endpoint);
          equal(served.outcome.kind, 'failed', 'a thrown Error in `before` did not fail it');
          equal(served.outcome.error.kind, 'internal', 'it was not `internal`');
          ok(
            served.outcome.error.detail.includes('layer 0 (auth)'),
            `the detail should name the layer: ${served.outcome.error.detail}`,
          );
          ok(
            served.outcome.error.detail.includes('no credentials'),
            `and the message: ${served.outcome.error.detail}`,
          );
        },
      );
    },
  },

  {
    name: 'a layer whose `after` throws refuses the exchange too',
    async run(api) {
      await withEndpoint(
        api,
        {
          answer: (request) => request,
          layers: [
            {
              name: 'audit',
              after: () => {
                throw api.unsupported('the audit log is read-only');
              },
            },
          ],
        },
        async (endpoint) => {
          const served = await serve(api, endpoint);
          equal(served.outcome.kind, 'failed', 'a failing `after` did not fail the exchange');
          equal(served.outcome.error.kind, 'unsupported', 'the chosen case was lost');
        },
      );
    },
  },

  {
    name: 'a layer with only `after` passes every request down, and one with only `before` passes every reply back',
    async run(api) {
      const seen = [];
      await withEndpoint(
        api,
        {
          answer: (request) => {
            seen.push('answer');
            return { ...request, content: 'answered' };
          },
          layers: [
            {
              name: 'outer',
              before: (request) => {
                seen.push('before:outer');
                return api.pass(request);
              },
            },
            {
              name: 'inner',
              after: (reply) => {
                seen.push('after:inner');
                return reply;
              },
            },
          ],
        },
        async (endpoint) => {
          const served = await serve(api, endpoint);
          equal(served.outcome.kind, 'ok', 'the exchange did not complete');
          equal(served.reply.content, 'answered', 'the handler’s reply was lost');
          // The order is the **engine's**: outermost first in, innermost first out. The
          // absent halves cost nothing and are simply not called.
          deepEqual(
            seen,
            ['before:outer', 'answer', 'after:inner'],
            `the composed order was ${seen.join(' → ')}`,
          );
        },
      );
    },
  },

  {
    name: 'an authority that throws is reported and answered with its safe default',
    async run(api) {
      const reported = [];
      await withEndpoint(
        api,
        {
          answer: (request) => request,
          authority: {
            isOwn: () => {
              throw new Error('the session store is down');
            },
            issued: () => {
              throw new Error('the session store is still down');
            },
          },
          onHostError: (error, where) => reported.push(`${where}: ${error.message}`),
        },
        async (endpoint) => {
          // The three authority answers are synchronous `func`s with no `result` in the
          // contract, so there is nothing to lower a failure into. Measured: a throw
          // that escapes here takes the whole exported call with it. The wrapper answers
          // the safe default instead and says so.
          const served = await serve(api, endpoint, WITH_TOKEN);
          equal(served.outcome.kind, 'ok', 'a throwing authority failed the exchange');
          ok(reported.length > 0, 'the throw was swallowed without a word');
          ok(
            reported.some((one) => one.includes('isOwn')),
            `the report should name the answer that threw: ${reported.join('; ')}`,
          );
        },
      );
    },
  },

  {
    name: 'a configuration read that throws is reported and answered with the engine’s default',
    async run(api) {
      const reported = [];
      // `rejection` is the deployment's own object rather than a copy of it — the other
      // three configuration blocks are merged into fresh objects — so a getter on it runs
      // inside `host-endpoint.rejection-policy()`, once per exported call. There is no
      // `result` in the contract for those four reads, so before this was wrapped a throw
      // here rejected `engine.serve` itself: no `Served`, no reply, and §6.1's obligation
      // lost to a host-side accident. Measured, not assumed.
      const rejection = {
        get inbound() {
          throw new Error('the deployment read its own config badly');
        },
        get outbound() {
          throw new Error('the deployment read its own config badly');
        },
      };
      await withEndpoint(
        api,
        {
          answer: (request) => request,
          rejection,
          onHostError: (error, where) => reported.push(`${where}: ${error.message}`),
        },
        async (endpoint) => {
          const served = await serve(api, endpoint, WITH_TOKEN);
          equal(served.outcome.kind, 'ok', 'a throwing configuration read failed the exchange');
          ok(served.reply !== undefined, 'every outcome still produces a reply');
          ok(reported.length > 0, 'the throw was swallowed without a word');
          ok(
            reported.some((one) => one.includes('rejection')),
            `the report should name the read that threw: ${reported.join('; ')}`,
          );
          // And the default that was used is the engine's own, not one written here: an
          // error still rejects, so the peer's token is echoed and the exchange is `ok`.
          const tokens = (served.reply.submessages ?? []).filter((one) => one.format === 'token');
          equal(tokens.length, 1, `expected the peer's token echoed, got ${tokens.length}`);
        },
      );
    },
  },

  {
    name: 'two clients with two transports are told apart by the exchange this package minted',
    async run(api) {
      const seen = [];
      const reply = (text) =>
        new TextEncoder().encode(
          JSON.stringify({ format: 'text', subformat: 'english', content: text }),
        );
      const client = (label, text) =>
        api.createClient({
          transport: {
            send: async (_bytes, cx) => {
              seen.push(`${label}@${cx.exchange}`);
              // Enough microtask turns for a registry scoped by "whichever call is
              // running" to lose track, if it were scoped that way.
              for (let turn = 0; turn < 40; turn += 1) await Promise.resolve();
              return { body: reply(text), detail: { kind: 'http', status: 200 } };
            },
          },
        });

      // Both are started before either is awaited. jco's task model serialises the two
      // exported calls today — the second `call-through` does not enter until the first
      // has returned — so what this proves is the *keying*, not overlap: each transport
      // is asked once, under its own number, and neither answers for the other. The
      // overlapping version is in `test/e2e/`, waiting on the same defect as the rest.
      const [first, second] = await Promise.all([
        client('one', 'from one').call(REQUEST),
        client('two', 'from two').call(REQUEST),
      ]);
      equal(first.text, 'from one', 'the first client got the second\u2019s transport');
      equal(second.text, 'from two', 'the second client got the first\u2019s transport');
      equal(seen.length, 2, `both transports should have been asked: ${seen.join(', ')}`);
      ok(
        new Set(seen.map((one) => one.split('@')[1])).size === 2,
        `the two calls should carry different exchange numbers: ${seen.join(', ')}`,
      );
      // And the numbers are this package's, not the guest's: above 2**32, so that a
      // reader can tell the two apart in a log line.
      for (const one of seen) {
        ok(
          BigInt(one.split('@')[1]) > 2n ** 32n,
          `an exchange this package minted should be above 2**32: ${one}`,
        );
      }
      return seen.join(', ');
    },
  },

  {
    name: 'a caller that names its own exchange gets it back in the transport',
    async run(api) {
      let saw;
      const client = api.createClient({
        transport: {
          send: (_bytes, cx) => {
            saw = cx;
            return {
              body: new TextEncoder().encode(
                '{"format":"text","subformat":"english","content":"ok"}',
              ),
              detail: { kind: 'http', status: 200 },
            };
          },
        },
      });
      await client.call(REQUEST, { exchange: 77n, peer: '10.0.0.1', correlator: 'demo-1' });
      equal(saw.exchange, 77n, 'the caller’s exchange number did not travel');
      equal(saw.peer, '10.0.0.1', 'the peer did not travel');
      equal(saw.correlator, 'demo-1', 'the correlator did not travel');
    },
  },

  {
    name: 'a transport that throws is `interrupted` unless it says otherwise',
    async run(api) {
      const anonymous = api.createClient({
        transport: {
          send: () => {
            throw new Error('the socket went away');
          },
        },
      });
      const vague = await throws(() => anonymous.call(REQUEST), 'a throwing transport should fail');
      equal(vague.error.kind, 'interrupted', 'an unknown failure must not claim `unreachable`');

      const precise = api.createClient({
        transport: {
          send: () => {
            throw api.unreachableError('https://example.test/nlip', 'connection refused');
          },
        },
      });
      const known = await throws(() => precise.call(REQUEST), 'a refused connection should fail');
      equal(known.error.kind, 'unreachable', 'a transport that knows was not believed');
      equal(known.error.endpoint, 'https://example.test/nlip', 'the end-point was lost');
    },
  },

  {
    name: 'one process installs one end-point, and says why',
    async run(api) {
      await withEndpoint(api, { answer: (request) => request }, async () => {
        const error = await throws(
          () => api.createEndpoint({ answer: (request) => request }),
          'a second end-point should be refused',
        );
        equal(error.code, 'already-installed', `expected already-installed, got ${error.code}`);
        ok(
          error.message.includes('host-endpoint.name()'),
          `the message should say what makes it so: ${error.message}`,
        );
      });
      // And the slot comes back, so a process that closes one can open another.
      await withEndpoint(api, { answer: (request) => request }, async (endpoint) => {
        equal(endpoint.name, 'nlipsy', 'the slot did not come back');
      });
    },
  },

  {
    name: 'one process installs one record sink, and says why',
    async run(api) {
      const first = api.createClient({ url: 'https://example.test/nlip', recordTo: { write: () => {} } });
      try {
        const error = await throws(
          async () =>
            api.createClient({ url: 'https://example.test/nlip', recordTo: { write: () => {} } }),
          'a second recording client should be refused',
        );
        equal(error.code, 'already-installed', `expected already-installed, got ${error.code}`);
        ok(
          error.message.includes('carries no context'),
          `the message should say what makes it so: ${error.message}`,
        );
      } finally {
        first.close();
      }
    },
  },

  {
    name: 'the engine asked for an end-point and there is none: the error says what to do',
    async run(api) {
      const error = await throws(
        () => api.corpus.check({ ...REQUEST }),
        'clause 6 without an end-point should say so',
      );
      equal(error.code, 'not-installed', `expected not-installed, got ${String(error)}`);
      ok(
        error.message.includes('createEndpoint'),
        `the message should name the fix: ${error.message}`,
      );
    },
  },
];
