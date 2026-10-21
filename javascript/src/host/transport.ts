/**
 * `nlipsy:engine/host-transport` — an alternative to `wasi:http/client` for the client
 * half.
 *
 * The target of `--map 'nlipsy:engine/host-transport=../host/transport.js'`. It is the
 * one of the five that is **keyed per call**, and the reason is in `registry.ts`:
 * `send` carries the `context`, every call that reaches it starts at
 * `client.call-through`, and this package is what makes that call — so it passes an
 * exchange number of its own minting and gets the same number back here. Five clients in
 * one process are five entries in a map, told apart exactly.
 *
 * A host that installed no transport is answered `unreachable`, which is the default a
 * Rust deployment gets from the trait and what a host with nothing to send through
 * should say — WIT has no optional imports, so the interface is implemented either way.
 */

import { fromWitContext, toWitExchange } from '../boundary.js';
import { transportFor } from '../registry.js';
import { throwTransportError } from './lower.js';
import { LoweredError } from '../errors.js';
import type * as wit from '../wasm/interfaces/nlipsy-engine-types.js';

/** What a transport that does not exist is called in the failure it reports. */
const NO_ENDPOINT = 'the host transport';

/**
 * Send `request` to the end-point and return what came back.
 *
 * Where it is going is the transport's business — the host that supplies one is the host
 * that knows — which is why there is no URL in the signature. The correlator is in `cx`.
 */
export async function send(request: Uint8Array, cx: wit.Context): Promise<wit.Exchange> {
  const transport = transportFor(cx.exchange);
  if (transport === undefined) {
    const detail =
      `no transport is installed for exchange ${cx.exchange}. ` +
      '`client.call-through` is reached only through a client built with a `transport`;' +
      ' a client without one calls over `wasi:http/client` instead.';
    throw new LoweredError(
      { tag: 'unreachable', val: { endpoint: NO_ENDPOINT, detail } },
      detail,
    );
  }
  try {
    return toWitExchange(await transport.send(request, fromWitContext(cx)));
  } catch (error) {
    throwTransportError(error, NO_ENDPOINT, 'the transport’s `send`');
  }
}
