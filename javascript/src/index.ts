/**
 * `nlipsy` — NLIP (ECMA-430) as a WebAssembly component, and the small amount of
 * JavaScript that gives it your code.
 *
 * ```js
 * import { serve, createClient, decode } from 'nlipsy';
 * ```
 *
 * Importing this module instantiates the component through jco's top-level `await`, so
 * everything below is callable the moment the import resolves. That is why the package
 * is ESM only: there is no top-level await in CommonJS.
 *
 * **What is in the component, and therefore not here:** the message model, clause 5's
 * rules, clause 6's exchange and the order it runs in, the composition of your layers
 * over your handler, and ECMA-431 §6 in both directions. **What is here:** a loader, five
 * adapters from your objects to the component's five imports, the conversion of values
 * across the boundary, and two pieces of state the contract deliberately does not keep —
 * a client's URL and a replay cursor.
 */

// The message model and clause 5.
export {
  decode,
  encode,
  extractText,
  firstSubmessageToken,
  isControl,
  tokenCount,
  tokenKindOf,
  uploadRequest,
  uploadResponse,
  validate,
} from './protocol.js';

// The engine's own defaults, and the rules a host answering outside the binding needs.
export {
  defaultConfig,
  defaultHttpOptions,
  defaultRejection,
  errorReply,
  owedBy,
  publicMessage,
  retryAfterOf,
} from './defaults.js';

// A deployment.
export { createEndpoint } from './endpoint.js';
export type { InstalledEndpoint } from './endpoint.js';
export { serve } from './serve.js';
export type { Server, ServeOptions } from './serve.js';

// An agent calling one.
export { createClient } from './client.js';
export type { Client, ClientOptions, ClientRoute } from './client.js';

// Clause 6's two steps on their own. Not for a deployment — see the module's own note.
export * as corpus from './corpus.js';

// What this package throws, and what a handler throws back.
export {
  AlreadyInstalledError,
  EndpointFailure,
  MalformedError,
  NlipError,
  RefusedError,
  ReplayError,
  TransportFailedError,
  interruptedError,
  malformedTransportError,
  refused,
  timedOut,
  unavailable,
  unreachableError,
  unsupported,
} from './errors.js';
export { NotInstalledError } from './host/lower.js';

// The two halves of a layer's answer.
export { answer, pass } from './types.js';

// The vocabulary.
export type {
  Body,
  Config,
  Content,
  ContentValue,
  Context,
  Cors,
  Disposition,
  Endpoint,
  EndpointError,
  Exchange,
  ExchangeDetail,
  Finding,
  Finished,
  Format,
  HttpOptions,
  Layer,
  LayerStep,
  Message,
  Outcome,
  Owed,
  ParseError,
  ParseErrorKind,
  Policy,
  RecordSink,
  Rejected,
  Rejection,
  Reply,
  Served,
  Severity,
  StatusMap,
  Submessage,
  TokenAuthority,
  TokenKind,
  Transport,
  TransportError,
  Violation,
  Wire,
} from './types.js';

// The one predicate the corpus runner shares with the boundary: which numbers a double
// rounds. Exported so that there is one copy of it rather than two.
export { isWideIntegerSource } from './boundary.js';
