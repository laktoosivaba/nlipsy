/**
 * An agent calling an end-point.
 *
 * Four routes, and the client's job is to pick one and to keep the two pieces of state
 * the contract deliberately does not: the URL and the replay cursor. Everything else —
 * the ECMA-431 §6 request, the correlator header, the content type, what a reply's text
 * is, which of its tokens are conversation tokens, whether it is an error — happens
 * inside the component.
 *
 * | given | route | what carries it |
 * |---|---|---|
 * | `url` | `client.call` | `wasi:http/client@0.3.0`, inside the component |
 * | `transport` | `client.call-through` | the user's object, through `host-transport` |
 * | `url` + `recordTo` | `client.call-recording` | as `call`, each exchange one JSON line |
 * | `replayFrom` | `client.replay` | a recording, with the cursor kept here |
 *
 * The combinations that are not in that table are **refused at construction** rather
 * than resolved by precedence: there is no contract function that records a call made
 * through a host transport, and a client that quietly dropped the `recordTo` would
 * produce a recording that stopped growing — the failure `wit/nlipsy.wit` names as the
 * reason `host-record-sink`'s failure is never swallowed.
 */

import { client as guest } from './glue.js';
import { asReplayFailed, asTransportFailed } from './lift.js';
import { fromWitReply, toWitContext, toWitMessage } from './boundary.js';
import { NlipError } from './errors.js';
import { installSink, mintExchange, registerTransport, reportToConsole } from './registry.js';
import type { HostErrorReporter } from './registry.js';
import type { Context, Message, RecordSink, Reply, Transport, Wire } from './types.js';

/** How this client's exchange is carried. */
export type ClientRoute = 'http' | 'transport' | 'recording' | 'replay';

/** What to build a client over. */
export interface ClientOptions {
  /** The whole URL a deployment published — not a host and a port with a path appended. */
  url?: string;
  /** Which encoding to write and to read. `json` by default. */
  wire?: Wire;
  /** Carry the exchange through this object instead of `wasi:http/client`. */
  transport?: Transport;
  /** Write each exchange to this sink, as one JSON line. Needs `url`. */
  recordTo?: RecordSink;
  /** Answer from this recording — JSON lines — instead of going anywhere. */
  replayFrom?: string;
  /** Where an exception the contract has no channel for is reported. */
  onHostError?: HostErrorReporter;
}

/** One client. */
export interface Client {
  /** Which of the four routes this client took. */
  readonly route: ClientRoute;
  /**
   * How far through the recording this client has read.
   *
   * `0` on every route but `replay`. The guest keeps no cursor — a recording is answered
   * in order and the host is what remembers where it is — so this is that memory. A call
   * the recording refuses does **not** advance it: the exchange at this index is still
   * the one owed, and skipping to the next would answer a question the recording was not
   * asked.
   */
  readonly cursor: number;
  /** One exchange. */
  call(request: Message, cx?: Context): Promise<Reply>;
  /** Release the record-sink slot, if this client claimed one. Idempotent. */
  close(): void;
}

/** Build a client over one of the four routes. */
export function createClient(options: ClientOptions): Client {
  const wire: Wire = options.wire ?? 'json';
  const onHostError = options.onHostError ?? reportToConsole;
  const route = routeOf(options);

  let cursor = 0;
  const releaseSink =
    route === 'recording' && options.recordTo !== undefined
      ? installSink(options.recordTo, onHostError)
      : () => {};

  return {
    route,
    get cursor() {
      return cursor;
    },
    async call(request: Message, cx?: Context): Promise<Reply> {
      const message = toWitMessage(request);
      switch (route) {
        case 'replay': {
          try {
            const reply = fromWitReply(
              await guest.replay(options.replayFrom as string, cursor, message, wire),
            );
            cursor += 1;
            return reply;
          } catch (error) {
            throw asReplayFailed(error);
          }
        }
        case 'transport': {
          // The exchange number is what tells this client's transport from another's,
          // and the caller may name one of its own — `context.exchange` of `0` means it
          // did not, and then one is minted here rather than by the guest, because the
          // guest's number would reach `host-transport.send` too late to look anything
          // up with (`registry.ts`).
          const named = cx?.exchange;
          const exchange = named === undefined || named === 0n ? mintExchange() : named;
          const release = registerTransport(exchange, options.transport as Transport);
          try {
            return fromWitReply(
              await guest.callThrough(message, wire, toWitContext({ ...cx, exchange })),
            );
          } catch (error) {
            throw asTransportFailed(error);
          } finally {
            release();
          }
        }
        case 'recording': {
          try {
            return fromWitReply(
              await guest.callRecording(options.url as string, message, wire, toWitContext(cx)),
            );
          } catch (error) {
            throw asTransportFailed(error);
          }
        }
        case 'http': {
          try {
            return fromWitReply(
              await guest.call(options.url as string, message, wire, toWitContext(cx)),
            );
          } catch (error) {
            throw asTransportFailed(error);
          }
        }
      }
    },
    close: releaseSink,
  };
}

/** Which route the options name, or why they name none. */
function routeOf(options: ClientOptions): ClientRoute {
  const named = [
    options.replayFrom !== undefined && 'replayFrom',
    options.transport !== undefined && 'transport',
    options.recordTo !== undefined && 'recordTo',
  ].filter((one): one is string => one !== false);

  if (options.replayFrom !== undefined) {
    if (named.length > 1) throw ambiguous(named);
    return 'replay';
  }
  if (options.transport !== undefined) {
    if (named.length > 1) throw ambiguous(named);
    return 'transport';
  }
  if (options.recordTo !== undefined) {
    if (options.url === undefined) throw needsUrl('recordTo');
    return 'recording';
  }
  if (options.url === undefined) {
    throw new NlipError(
      'client-options',
      'nlipsy: a client needs one of `url`, `transport` or `replayFrom` — where to send ' +
        'the request, what to send it through, or what to answer it from.',
    );
  }
  return 'http';
}

function ambiguous(named: string[]): NlipError {
  return new NlipError(
    'client-options',
    `nlipsy: a client takes one of \`replayFrom\`, \`transport\` and \`recordTo\`, not ` +
      `${named.join(' and ')}. The contract has one function per route and none that ` +
      'combines them, so there is no honest way to do both at once.',
  );
}

function needsUrl(what: string): NlipError {
  return new NlipError(
    'client-options',
    `nlipsy: \`${what}\` records what went out over HTTP, so it needs a \`url\` to send ` +
      'the request to.',
  );
}
