/**
 * A browser client over a `fetch` transport.
 *
 * Serve this directory with anything that resolves a bare specifier, serves `.wasm` as
 * `application/wasm` and compiles TypeScript — a Vite dev server over the package does
 * all three — and open it. It is `.ts` rather than `.js` so that it is type-checked
 * against the package's own types by `npm run typecheck`, which is what makes it
 * evidence about the API rather than a listing.
 *
 * **It does not answer yet.** A `fetch` is an await on the event loop, and a host
 * callback suspended across one traps the component under jco 1.39.0
 * (`test/e2e/reason.mjs`). The code below is what a browser client *is*, and it is
 * three lines of transport: no status code, no header, no NLIP framing. The request
 * bytes are the component's, written on the wire the client was built with; the reply is
 * whatever came back, decoded by the component, with its text and its tokens already
 * derived.
 */

import { createClient, interruptedError, unreachableError } from 'nlipsy';
import type { Context, Submessage, Transport } from 'nlipsy';

/**
 * The browser's transport.
 *
 * The three cases of a transport failure are not interchangeable and only the transport
 * knows which one happened: a request that never left cannot have been seen by the
 * end-point, and one that was cut off mid-reply may well have been. `fetch` rejects with
 * a `TypeError` for the first and an `AbortError` for the second, so this says which —
 * a transport that threw anything else is reported as `interrupted`, which is the safe
 * answer and not a guess: promising that the peer never saw the request is the promise
 * that invites a duplicate turn into somebody's transcript.
 */
function fetchTransport(url: string): Transport {
  return {
    async send(request: Uint8Array, cx: Context) {
      const headers: Record<string, string> = { 'content-type': 'application/json' };
      if (cx.correlator !== undefined) headers['x-nlip-correlator'] = cx.correlator;
      let response;
      try {
        // `body` takes a `BufferSource`, and a `Uint8Array` over a possibly-shared
        // buffer is not one as far as the DOM types are concerned. The bytes are the
        // component's and are copied nowhere: this is the view's own buffer, sliced to
        // the view.
        const body = request.buffer.slice(
          request.byteOffset,
          request.byteOffset + request.byteLength,
        ) as ArrayBuffer;
        response = await fetch(url, { method: 'POST', headers, body });
      } catch (error) {
        if (error instanceof Error && error.name === 'AbortError') {
          throw interruptedError(url, 'the request was aborted', error);
        }
        throw unreachableError(url, error instanceof Error ? error.message : String(error), error);
      }
      const retryAfter = response.headers.get('retry-after');
      return {
        body: new Uint8Array(await response.arrayBuffer()),
        detail: {
          kind: 'http' as const,
          status: response.status,
          // Seconds on the wire, milliseconds across the boundary. The *rule* about what
          // a `Retry-After` means is the engine's; this is a unit.
          ...(retryAfter !== null && /^\d+$/.test(retryAfter)
            ? { retryAfterMs: Number(retryAfter) * 1000 }
            : {}),
        },
      };
    },
  };
}

const out = document.querySelector('#out') as HTMLElement;
const urlField = document.querySelector('#url') as HTMLInputElement;
const sayField = document.querySelector('#say') as HTMLInputElement;
let conversation: Submessage[] | undefined;

document.querySelector('#send')?.addEventListener('click', async () => {
  const url = urlField.value;
  const client = createClient({ transport: fetchTransport(url) });
  try {
    const reply = await client.call({
      format: 'text',
      subformat: 'english',
      content: sayField.value,
      // §6.2 ¶2's obligation runs both ways: what came back last time goes out this
      // time. `reply.tokens` is what discharges it — the submessages exactly as they
      // arrived — and the engine is what decided which those are.
      ...(conversation === undefined ? {} : { submessages: conversation }),
    });
    conversation = reply.tokens;
    out.textContent = [
      `status ${reply.detail.status}${reply.isError ? ' (an error in NLIP terms)' : ''}`,
      reply.text ?? '(no text)',
      reply.conversationTokens.length > 0
        ? `conversation: ${reply.conversationTokens.join(', ')}`
        : '',
    ]
      .filter(Boolean)
      .join('\n');
  } catch (error) {
    const named = error as { code?: string; message?: string };
    out.textContent = `${named.code ?? 'error'}: ${named.message ?? String(error)}`;
  }
});
