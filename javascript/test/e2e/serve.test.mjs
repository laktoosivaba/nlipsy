/**
 * An end-point on a socket: `serve()` over the component's own `wasi:http/handler`.
 *
 * Every assertion below is about the **component's** ECMA-431 §6 binding — the path, the
 * method, the status line, `Allow: POST`, the correlator echoed, the 404 body — because
 * that is where all of it lives. Nothing in this package writes a header.
 */
import { describe, it, expect } from 'vitest';

import { api } from '../spec/index.mjs';
import { BLOCKERS, RUN, announce } from './reason.mjs';

announce('serve()', BLOCKERS.handler, BLOCKERS.eventLoop);

const PORT = 18_431;

describe.skipIf(!RUN)('serve()', () => {
  it('answers a POST on /nlip with the end-point’s reply and the peer’s correlator', async () => {
    const server = await api.serve({
      name: 'e2e',
      port: PORT,
      answer: (request) => ({ ...request, content: `you said: ${request.content}` }),
    });
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/nlip`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-nlip-correlator': 'demo-1' },
        body: '{"format":"text","subformat":"english","content":"What is Ecma?"}',
      });
      expect(response.status).toBe(200);
      expect(response.headers.get('x-nlip-correlator')).toBe('demo-1');
      const reply = await api.decode(new Uint8Array(await response.arrayBuffer()), 'json');
      expect(reply.content).toBe('you said: What is Ecma?');
    } finally {
      await server.close();
    }
  });

  it('answers another method with 405 and `Allow: POST`, and another path with 404', async () => {
    const server = await api.serve({ port: PORT, answer: (request) => request });
    try {
      const wrongMethod = await fetch(`http://127.0.0.1:${PORT}/nlip`);
      expect(wrongMethod.status).toBe(405);
      expect(wrongMethod.headers.get('allow')).toBe('POST');
      const body = await wrongMethod.json();
      expect(body.messagetype ?? body.messageType).toBe('Error');

      const wrongPath = await fetch(`http://127.0.0.1:${PORT}/nope`, { method: 'POST', body: '{}' });
      expect(wrongPath.status).toBe(404);
    } finally {
      await server.close();
    }
  });

  it('mints a correlator when the peer brought none', async () => {
    const server = await api.serve({ port: PORT, answer: (request) => request });
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/nlip`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: '{"format":"text","subformat":"english","content":"hi"}',
      });
      expect(response.status).toBe(200);
      // A UUID v4, as `ServerConfig::correlator_for` spells one. The rule and the
      // randomness are both the component's — `wasi:random/random@0.3.0`.
      expect(response.headers.get('x-nlip-correlator')).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
      );
    } finally {
      await server.close();
    }
  });

  it('refuses a body over the limit with the status the end-point configured', async () => {
    const server = await api.serve({
      port: PORT,
      http: { maxRequestBodySize: 64 },
      answer: (request) => request,
    });
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/nlip`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          format: 'text',
          subformat: 'english',
          content: 'x'.repeat(4096),
        }),
      });
      expect(response.status).toBe(413);
    } finally {
      await server.close();
    }
  });
});
