/**
 * ECMA-430 clause 5, as pure functions over the engine.
 *
 * Every one of these is one call into the component with a conversion on each side, and
 * that is deliberate to the point of being the package's whole thesis: which submessages
 * are tokens is a comparison on a field and a caller can write it, but *which reserved
 * kind a token is* is a rule (§5.3 read through §6.2), *what counts as control* is a
 * rule (§5.1.1 and §6.3 OR-ed), and *what a message's text is* is a rule (§5.1.5's
 * order). None of them is re-derived here.
 *
 * All async, because jco's output returns a promise from every export of this artefact —
 * see `glue.ts`.
 */

import { protocol } from './glue.js';
import { asMalformed } from './lift.js';
import {
  fromWitMessage,
  fromWitSubmessage,
  fromWitViolation,
  toWitMessage,
  toWitSubmessage,
} from './boundary.js';
import type { Message, Submessage, TokenKind, Violation, Wire } from './types.js';

/**
 * Decode one document.
 *
 * `wire` is a parameter and not a guess, so that ECMA-432 §7.2's text fallback stays the
 * caller's decision: a JSON string under `format: "binary"` is base64 and a CBOR text
 * string under the same format is not, and only the frame that carried it knows.
 *
 * @throws MalformedError carrying the `kind`, `path` and `field` the corpus compares.
 *         Peer input cannot trap the component; this is the boundary's own convention
 *         for the `result` the contract declares.
 */
export async function decode(bytes: Uint8Array, wire: Wire = 'json'): Promise<Message> {
  try {
    return fromWitMessage(await protocol.decode(bytes, wire));
  } catch (error) {
    throw asMalformed(error);
  }
}

/**
 * Encode one message.
 *
 * No failure mode from the engine: a message's keys are strings and its numbers are
 * finite. The conversion on this side can still refuse — a non-finite number, or content
 * JSON has no text for — and says so with a `TypeError` that names this package.
 *
 * On the JSON wire the bytes are the corpus's `canonical_json` exactly: fixed field
 * order, sorted keys inside structured content, padded base64, compact.
 */
export async function encode(message: Message, wire: Wire = 'json'): Promise<Uint8Array> {
  return await protocol.encode(toWitMessage(message), wire);
}

/**
 * Apply clause 5's semantic rules.
 *
 * Findings, not failures: the message decoded. The corpus compares these as a set of
 * `{rule, severity, path}`; the order is not part of any expectation.
 */
export async function validate(message: Message): Promise<Violation[]> {
  return (await protocol.validate(toWitMessage(message))).map(fromWitViolation);
}

/**
 * `true` when `control` is `true` **or** `messageType` equals `control` ignoring case.
 *
 * ECMA-430 is inconsistent here — §5.1.1 reserves the *value* while §6.3 speaks of a
 * boolean *field* — and both are read and OR-ed. It is a rule, which is why it is a call.
 */
export async function isControl(message: Message): Promise<boolean> {
  return await protocol.isControl(toWitMessage(message));
}

/**
 * All `text` content of the message in `language`, joined by single spaces in document
 * order.
 *
 * `undefined` for `language` collects every text subformat. `undefined` comes back when
 * there is no text at all, which is not the same as an empty string.
 */
export async function extractText(
  message: Message,
  language?: string,
): Promise<string | undefined> {
  return await protocol.extractText(toWitMessage(message), language);
}

/**
 * The message's *first* submessage as a submessage, when that submessage is a token.
 *
 * §5 ¶2 calls the leading object "the first submessage", so a message that *is* a token
 * carries one for §6.2's purposes — and `message.submessages` cannot show it, which is
 * why this exists.
 */
export async function firstSubmessageToken(message: Message): Promise<Submessage | undefined> {
  const found = await protocol.firstSubmessageToken(toWitMessage(message));
  return found === undefined ? undefined : fromWitSubmessage(found);
}

/**
 * How many `token` submessages a request carries, the first submessage included.
 *
 * Counted rather than collected: it runs before §6.2's ceiling is applied, so a message
 * with a hundred thousand tokens does not cost a hundred thousand copies to refuse.
 */
export async function tokenCount(message: Message): Promise<number> {
  return await protocol.tokenCount(toWitMessage(message));
}

/**
 * Which reserved kind a token submessage is, or `undefined` for the application's own.
 *
 * The subformat's *prefix*, matched case-insensitively: §5.3 reserves exactly two.
 * `authorization`, which the reference Python SDK emits, is application-defined and
 * answers `undefined` — echoed like any other token, but not counted as a credential.
 */
export async function tokenKindOf(submessage: Submessage): Promise<TokenKind | undefined> {
  return await protocol.tokenKindOf(toWitSubmessage(submessage));
}

/** The §6.4 out-of-band *request*: a control message asking for an upload target. */
export async function uploadRequest(ask: string): Promise<Message> {
  return fromWitMessage(await protocol.uploadRequest(ask));
}

/**
 * The §6.4 out-of-band *response*.
 *
 * The shape is easy to build wrongly and is named once in the contract rather than
 * assembled at every call site: a control message **containing a submessage** whose
 * format is `structured` and whose subformat is `uri`, not a bare `structured`/`uri`
 * message.
 */
export async function uploadResponse(uri: string): Promise<Message> {
  return fromWitMessage(await protocol.uploadResponse(uri));
}
