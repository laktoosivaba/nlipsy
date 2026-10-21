// An end-point on a socket, under Bun. Run by `test/e2e/runtimes.test.mjs`.
//
// Bun runs the package itself — `test/node/bun.test.mjs` puts the whole corpus through
// it today — but it cannot *serve*: `HttpServer.listen` moves a `ReadableStream` and a
// `MessagePort` into a `node:worker_threads` worker and Bun refuses the transfer list
// (`DOMException [DataCloneError]`). Printing the failure is the test.
import * as api from '../../../dist/index.js';

const port = Number(process.argv[2] ?? 18_432);
const server = await api.serve({ port, answer: (request) => request });
const response = await fetch(`http://127.0.0.1:${port}/nlip`, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: '{"format":"text","subformat":"english","content":"hi"}',
});
console.log(`status ${response.status}`);
await server.close();
