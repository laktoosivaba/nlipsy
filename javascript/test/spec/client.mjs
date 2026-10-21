/**
 * The client's four routes, the one piece of state it keeps, and the shape of a reply.
 *
 * Everything a reply *means* — its text, which of its tokens are conversation tokens,
 * whether it is an error — is the engine's and is a field on the value the component
 * returned. What is asserted here is that the wrapper picks the right route, keeps the
 * cursor honestly, and carries all five derived views across unchanged.
 */

import { deepEqual, equal, ok, throws } from './assert.mjs';

const REQUEST = { format: 'text', subformat: 'english', content: 'What is Ecma?' };

/** One line of a recording, in the spelling `Recorded::to_json_line` writes. */
function recorded(request, body, status = 200) {
  return JSON.stringify({ request: JSON.stringify(request), status, body: JSON.stringify(body) });
}

const RECORDING = [
  recorded(REQUEST, { format: 'text', subformat: 'english', content: 'the first answer' }),
  recorded(
    { format: 'text', subformat: 'english', content: 'And ECMA-430?' },
    { format: 'text', subformat: 'english', content: 'the second answer' },
  ),
].join('\n');

export const specs = [
  {
    name: 'each set of options names one route, and the combinations that name none are refused',
    async run(api) {
      equal(api.createClient({ url: 'https://example.test/nlip' }).route, 'http', 'url');
      equal(api.createClient({ transport: { send: () => ({}) } }).route, 'transport', 'transport');
      equal(api.createClient({ replayFrom: RECORDING }).route, 'replay', 'replayFrom');

      const recording = api.createClient({
        url: 'https://example.test/nlip',
        recordTo: { write: () => {} },
      });
      equal(recording.route, 'recording', 'url + recordTo');
      recording.close();

      // The contract has one function per route and none that combines them, so a client
      // that quietly dropped one of two would produce a recording that stopped growing.
      const both = await throws(
        async () => api.createClient({ transport: { send: () => ({}) }, recordTo: { write: () => {} } }),
        'a transport and a recording together should be refused',
      );
      equal(both.code, 'client-options', `expected client-options, got ${String(both)}`);

      const nowhere = await throws(
        async () => api.createClient({}),
        'a client with no route should be refused',
      );
      equal(nowhere.code, 'client-options', `expected client-options, got ${String(nowhere)}`);

      const unrecordable = await throws(
        async () => api.createClient({ recordTo: { write: () => {} } }),
        'a recording with nowhere to send should be refused',
      );
      equal(unrecordable.code, 'client-options', `expected client-options, got ${String(unrecordable)}`);
    },
  },

  {
    name: 'a recording is answered in order, and the cursor is the host’s',
    async run(api) {
      const client = api.createClient({ replayFrom: RECORDING });
      equal(client.cursor, 0, 'a fresh client has read nothing');

      const first = await client.call(REQUEST);
      equal(first.text, 'the first answer', 'the first exchange was not the first line');
      equal(client.cursor, 1, 'the cursor did not advance');

      const second = await client.call({
        format: 'text',
        subformat: 'english',
        content: 'And ECMA-430?',
      });
      equal(second.text, 'the second answer', 'the second exchange was not the second line');
      equal(client.cursor, 2, 'the cursor did not advance again');

      // A recording that has run out refuses rather than answering from nowhere — and
      // the cursor does **not** move, because the exchange at this index is still the one
      // owed.
      const exhausted = await throws(
        () => client.call(REQUEST),
        'a recording with nothing left should refuse',
      );
      equal(exhausted.code, 'replay', `expected a replay failure, got ${String(exhausted)}`);
      equal(client.cursor, 2, 'a refused call moved the cursor');
    },
  },

  {
    name: 'a recording refuses a request it is not for, rather than answering the next one',
    async run(api) {
      const client = api.createClient({ replayFrom: RECORDING });
      const wrong = await throws(
        () => client.call({ format: 'text', subformat: 'english', content: 'something else' }),
        'a recording should refuse a request it has no exchange for',
      );
      equal(wrong.code, 'replay', `expected a replay failure, got ${String(wrong)}`);
      equal(client.cursor, 0, 'a refused call moved the cursor');
    },
  },

  {
    name: 'every derived view of a reply comes across, and none of them is recomputed here',
    async run(api) {
      const body = {
        format: 'text',
        subformat: 'english',
        content: 'An international standards body.',
        submessages: [
          { format: 'token', subformat: 'conversation', content: 'srv-1' },
          { format: 'token', subformat: 'authentication_bearer', content: 'cred-1' },
          { format: 'token', subformat: 'authorization', content: 'app-1' },
          { format: 'text', subformat: 'english', content: 'And it publishes ECMA-430.' },
        ],
      };
      const client = api.createClient({
        transport: {
          send: () => ({
            body: new TextEncoder().encode(JSON.stringify(body)),
            detail: { kind: 'http', status: 200 },
          }),
        },
      });
      const reply = await client.call(REQUEST);

      equal(reply.detail.kind, 'http', 'the binding detail was lost');
      equal(reply.detail.status, 200, 'the status was lost');
      equal(reply.detail.retryAfterMs, undefined, 'a `Retry-After` was invented');
      equal(reply.bytes, JSON.stringify(body).length, 'the byte count is not the body’s');
      equal(reply.body.kind, 'message', 'the body did not decode');
      equal(
        reply.text,
        'An international standards body. And it publishes ECMA-430.',
        '§5.1.5’s order was not preserved',
      );
      equal(reply.isError, false, 'a plain reply was read as an error');
      // Three tokens, the application's `authorization` among them: §6.2 ¶2 obliges the
      // echo of every token, and which ones are *reserved* is a rule the engine applies.
      equal(reply.tokens.length, 3, `expected three token submessages, got ${reply.tokens.length}`);
      deepEqual(reply.conversationTokens, ['srv-1'], 'the conversation token key');
      deepEqual(reply.authenticationTokens, ['cred-1'], 'the authentication token key');
      ok(
        reply.tokens.some((one) => one.subformat === 'authorization'),
        'the application’s own token was dropped from the echo',
      );
    },
  },

  {
    name: 'a body that is not a message is opaque rather than an error of ours',
    async run(api) {
      const client = api.createClient({
        transport: {
          send: () => ({
            body: new TextEncoder().encode('<html>502 Bad Gateway</html>'),
            detail: { kind: 'http', status: 502, retryAfterMs: 0 },
          }),
        },
      });
      const reply = await client.call(REQUEST);
      equal(reply.body.kind, 'opaque', 'a proxy’s HTML was read as a message');
      ok(reply.body.text.includes('502'), 'the preview a user has to see was lost');
      equal(reply.isError, true, 'a body that never spoke NLIP was not counted as an error');
      // `some(0)` and `none` are different answers, and this is the one that proves the
      // boundary keeps them apart.
      equal(reply.detail.retryAfterMs, 0, 'a `Retry-After` of zero became absent');
    },
  },

  {
    name: 'a recording written by this package is one the engine reads back',
    async run(api) {
      // The point is not that a line round-trips — it is that the spelling is the
      // **engine's**: `Recorded::to_json_line` writes it inside the component, and
      // `replay` inside the component reads it. Nothing in this package writes or parses
      // a recording, which is why a fixture recorded from JavaScript loads in Rust.
      const client = api.createClient({ replayFrom: RECORDING });
      const reply = await client.call(REQUEST);
      equal(reply.text, 'the first answer', 'the engine did not read the line');
      equal(reply.detail.status, 200, 'the recorded status was lost');
    },
  },
];
