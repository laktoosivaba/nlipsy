/**
 * A client over `wasi:http/client@0.3.0` — the route `createClient({ url })` takes.
 *
 * The request it writes is ECMA-431 §6's, written inside the component: `POST`, the
 * content type, the correlator header. What is asserted here is what the peer *saw*,
 * because that is the only place the binding is observable from.
 */
import http from 'node:http';
import { describe, it, expect } from 'vitest';

import { api } from '../spec/index.mjs';
import { BLOCKERS, RUN, announce } from './reason.mjs';

announce('client over wasi:http', BLOCKERS.client);

/** A one-request end-point that records what it was sent. */
async function listener(reply, status = 200) {
  const seen = [];
  const server = http.createServer((request, response) => {
    let body = '';
    request.on('data', (chunk) => (body += chunk));
    request.on('end', () => {
      seen.push({ method: request.method, url: request.url, headers: request.headers, body });
      response.writeHead(status, { 'content-type': 'application/json' });
      response.end(JSON.stringify(reply));
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return { seen, port, close: () => new Promise((resolve) => server.close(resolve)) };
}

describe.skipIf(!RUN)('createClient({ url })', () => {
  it('writes ECMA-431 §6’s request and reads the reply the engine decoded', async () => {
    const peer = await listener({
      format: 'text',
      subformat: 'english',
      content: 'An international standards body.',
      submessages: [{ format: 'token', subformat: 'conversation', content: 'srv-1' }],
    });
    try {
      const client = api.createClient({ url: `http://127.0.0.1:${peer.port}/nlip` });
      const reply = await client.call(
        { format: 'text', subformat: 'english', content: 'What is Ecma?' },
        { exchange: 0n, correlator: 'demo-1' },
      );
      expect(reply.detail.status).toBe(200);
      expect(reply.text).toBe('An international standards body.');
      expect(reply.conversationTokens).toEqual(['srv-1']);

      const [request] = peer.seen;
      expect(request.method).toBe('POST');
      expect(request.url).toBe('/nlip');
      expect(request.headers['content-type']).toBe('application/json');
      expect(request.headers['x-nlip-correlator']).toBe('demo-1');
      // Set by the guest so that a peer applying its own limit has something to read.
      expect(request.headers['content-length']).toBe(String(request.body.length));
    } finally {
      await peer.close();
    }
  });

  it('records what it saw in the spelling `replay` reads back', async () => {
    const peer = await listener({
      format: 'text',
      subformat: 'english',
      content: 'the recorded answer',
    });
    const lines = [];
    try {
      const recording = api.createClient({
        url: `http://127.0.0.1:${peer.port}/nlip`,
        recordTo: { write: (line) => lines.push(line) },
      });
      const request = { format: 'text', subformat: 'english', content: 'What is Ecma?' };
      const live = await recording.call(request);
      expect(live.text).toBe('the recorded answer');
      recording.close();
      expect(lines).toHaveLength(1);

      // And the same recording answers the same request with no peer at all — which is
      // the whole point of a fixture, and is the same text a Rust test loads.
      const replaying = api.createClient({ replayFrom: lines.join('\n') });
      const replayed = await replaying.call(request);
      expect(replayed.text).toBe('the recorded answer');
      expect(replaying.cursor).toBe(1);
    } finally {
      await peer.close();
    }
  });
});
