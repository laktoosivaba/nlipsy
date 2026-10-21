/**
 * The conformance corpus, run against this package.
 *
 * `conformance/cases/` is the truth about behaviour and it lives above every package, so
 * this file is a *harness* and not a second corpus: every expectation it compares is the
 * one `crates/conformance/src/{lib,exchange,component}.rs` compares, in the same order,
 * with the same defaults, and `conformance/README.md`'s language-neutral algorithm is
 * what both are a reading of. A runner that computed an expectation of its own would be
 * this package marking its own paper.
 *
 * It mirrors `crates/conformance/src/component.rs` in particular, which is the same
 * corpus through the same artefact with a wasmtime host in place of this one. Where that
 * file calls `protocol.call_validate`, this one calls `validate`; where it drives
 * `engine.check` and `engine.finish`, this one drives `corpus.check` and `corpus.finish`;
 * where it puts a `short_circuit` case through a host layer whose `before` answers and
 * then `engine.serve-message`, so does this. Nothing else differs, which is the point.
 *
 * Plain ESM with no test framework in it, so that a Bun or a browser harness can name the
 * same module on the day either can run it.
 */

import { isWideIntegerSource } from '../dist/index.js';

// ---------------------------------------------------------------------------
// Reading a case
// ---------------------------------------------------------------------------

/**
 * Parse a case file, keeping an integer JSON cannot hold in a `number`.
 *
 * `ecma-432/12-cbor-bignum-content` asserts `18446744073709551615`, which is `u64::MAX`
 * and is 2 048 away from the nearest `double`. The engine carries it exactly — the
 * contract's `number` is a three-way variant precisely so that it can — so a harness
 * that let `JSON.parse` round the *expectation* would compare a correct answer against a
 * corrupted one and report the engine as wrong.
 *
 * The third reviver argument is ES2025's source access. Where it is missing this throws
 * rather than falling back, because the fallback is silent wrong data.
 *
 * `isWideIntegerSource` is the package's own predicate, imported rather than repeated:
 * the wrapper reads the same numbers out of `structured` content with it, and two copies
 * of "which numbers a double rounds" would be two answers waiting to disagree.
 */
export function parseCase(text) {
  let sawSource = false;
  const parsed = JSON.parse(text, function (_key, value, context) {
    if (context !== undefined && typeof context === 'object') sawSource = true;
    if (isWideIntegerSource(value, context)) return BigInt(context.source);
    return value;
  });
  if (!sawSource) {
    throw new Error(
      "this runtime does not give JSON.parse's reviver the source text, so a corpus " +
        'case carrying an integer wider than a double cannot be read exactly',
    );
  }
  return parsed;
}

// ---------------------------------------------------------------------------
// The corpus's canonical spelling of a value
// ---------------------------------------------------------------------------

const BASE64 =
  typeof btoa === 'function'
    ? (bytes) => {
        let binary = '';
        for (const byte of bytes) binary += String.fromCharCode(byte);
        return btoa(binary);
      }
    : (bytes) => Buffer.from(bytes).toString('base64');

/**
 * A content value in the corpus's model spelling.
 *
 * The only difference from what JSON would write is `Uint8Array`: JSON spells a byte
 * string as base64 (§6.4), which is indistinguishable from a string that happens to look
 * like base64, so the model wraps it in `{"$bytes": …}`. That is the distinction the
 * whole marker exists for.
 */
export function canonicalContent(content) {
  if (content instanceof Uint8Array) return { $bytes: BASE64(content) };
  if (Array.isArray(content)) return content.map(canonicalContent);
  if (content !== null && typeof content === 'object') {
    const out = {};
    for (const [key, value] of Object.entries(content)) out[key] = canonicalContent(value);
    return out;
  }
  return content;
}

/**
 * A decoded message in the corpus's model spelling.
 *
 * Absent optional fields are omitted rather than spelled `null`, because `content: null`
 * is a value Annex A allows and an absent field is not the same thing.
 */
export function canonicalModel(message) {
  const out = {};
  if (message.messageType !== undefined) out.message_type = message.messageType;
  if (message.control !== undefined) out.control = message.control;
  if (message.label !== undefined) out.label = message.label;
  out.format = message.format;
  out.subformat = message.subformat;
  out.content = canonicalContent(message.content);
  const submessages = message.submessages ?? [];
  if (submessages.length > 0) {
    out.submessages = submessages.map((submessage) => {
      const part = {};
      if (submessage.label !== undefined) part.label = submessage.label;
      part.format = submessage.format;
      part.subformat = submessage.subformat;
      part.content = canonicalContent(submessage.content);
      return part;
    });
  }
  return out;
}

/** Structural equality, with `bigint` and `number` kept apart and `Uint8Array` by bytes. */
export function same(left, right) {
  if (left === right) return true;
  if (typeof left !== typeof right) return false;
  if (left === null || right === null) return false;
  if (left instanceof Uint8Array || right instanceof Uint8Array) {
    if (!(left instanceof Uint8Array) || !(right instanceof Uint8Array)) return false;
    if (left.length !== right.length) return false;
    for (let at = 0; at < left.length; at += 1) if (left[at] !== right[at]) return false;
    return true;
  }
  if (Array.isArray(left) || Array.isArray(right)) {
    if (!Array.isArray(left) || !Array.isArray(right)) return false;
    if (left.length !== right.length) return false;
    return left.every((one, at) => same(one, right[at]));
  }
  if (typeof left === 'object') {
    const leftKeys = Object.keys(left).sort();
    const rightKeys = Object.keys(right).sort();
    if (leftKeys.length !== rightKeys.length) return false;
    if (!leftKeys.every((key, at) => key === rightKeys[at])) return false;
    return leftKeys.every((key) => same(left[key], right[key]));
  }
  return false;
}

/** One value on one line, so a difference reads as a line of output. */
export function compact(value) {
  return JSON.stringify(value, (_key, one) => (typeof one === 'bigint' ? `${one}` : one));
}

/** A hex string as bytes — the corpus's `input_hex`. */
export function fromHex(text) {
  if (text.length % 2 !== 0) throw new Error(`input_hex has an odd length (${text.length})`);
  const bytes = new Uint8Array(text.length / 2);
  for (let at = 0; at < bytes.length; at += 1) {
    const byte = Number.parseInt(text.slice(at * 2, at * 2 + 2), 16);
    if (Number.isNaN(byte)) throw new Error(`input_hex is not hex at byte ${at}`);
    bytes[at] = byte;
  }
  return bytes;
}

// ---------------------------------------------------------------------------
// A message case
// ---------------------------------------------------------------------------

/**
 * Everything a message case asks, as this package answered it.
 *
 * Six calls into the engine — `validate`, `isControl`, `extractText`, `encode` and
 * `tokenKindOf` beside the `decode` that produced `message`. Deriving any of them in
 * JavaScript instead would be the wrapper answering for the engine.
 */
async function observe(api, message) {
  const tokens = [];
  for (const submessage of message.submessages ?? []) {
    // The `token` filter is a comparison on a field the contract carries, not a rule;
    // the *classification* is `tokenKindOf`, which is one, and is asked of the engine.
    if (submessage.format !== 'token') continue;
    tokens.push({
      subformat: submessage.subformat,
      content: canonicalContent(submessage.content),
      kind: (await api.tokenKindOf(submessage)) ?? 'other',
    });
  }
  return {
    model: canonicalModel(message),
    violations: (await api.validate(message)).map(({ rule, severity, path }) => ({
      rule,
      severity,
      path,
    })),
    isControl: await api.isControl(message),
    text: await api.extractText(message, undefined),
    tokens,
    canonicalJson: new TextDecoder().decode(await api.encode(message, 'json')),
  };
}

function compareDescription(kase, described, out) {
  const expected = kase.expect.error;
  if (expected === undefined) {
    out.push('expect.decode is `error` with no `error` object');
    return;
  }
  if (described.kind !== expected.kind) {
    out.push(`error.kind: expected \`${expected.kind}\`, got \`${described.kind}\``);
  }
  if (described.path !== expected.path) {
    out.push(`error.path: expected \`${expected.path}\`, got \`${described.path}\``);
  }
  const wantedField = expected.field ?? undefined;
  if (described.field !== wantedField) {
    out.push(`error.field: expected ${compact(wantedField)}, got ${compact(described.field)}`);
  }
}

function compareObserved(kase, observed, out) {
  if ('message' in kase.expect) {
    if (!same(observed.model, kase.expect.message)) {
      out.push(`message: expected ${compact(kase.expect.message)}, got ${compact(observed.model)}`);
    }
  } else {
    out.push('expect.decode is `ok` with no `message`');
  }

  if ('violations' in kase.expect) {
    // §5.3's findings have no order the standard gives them, so they are compared as a
    // set: an implementation that reports the submessages first is not thereby wrong.
    const key = (one) => `${one.severity} ${one.rule} at ${one.path}`;
    const wanted = kase.expect.violations.map(key).sort();
    const actual = observed.violations.map(key).sort();
    if (!same(wanted, actual)) {
      out.push(`violations: expected ${compact(wanted)}, got ${compact(actual)}`);
    }
  } else {
    out.push('expect.decode is `ok` with no `violations`');
  }

  if ('is_control' in kase.expect) {
    if (observed.isControl !== kase.expect.is_control) {
      out.push(`is_control: expected ${kase.expect.is_control}, got ${observed.isControl}`);
    }
  } else {
    out.push('expect.decode is `ok` with no `is_control`');
  }

  if ('text' in kase.expect) {
    // An explicit `null` is "this message carries no text at all"; an absent key is a
    // malformed case. The two are different, which is why this reads the key.
    const wanted = kase.expect.text === null ? undefined : kase.expect.text;
    if (observed.text !== wanted) {
      out.push(`text: expected ${compact(wanted)}, got ${compact(observed.text)}`);
    }
  } else {
    out.push('expect.decode is `ok` with no `text`');
  }

  if ('tokens' in kase.expect) {
    const wanted = kase.expect.tokens.map(({ subformat, content, kind }) => ({
      subformat,
      content,
      kind,
    }));
    if (!same(observed.tokens, wanted)) {
      out.push(`tokens: expected ${compact(wanted)}, got ${compact(observed.tokens)}`);
    }
  }

  if ('canonical_json' in kase.expect) {
    if (observed.canonicalJson !== kase.expect.canonical_json) {
      out.push(
        `canonical_json: expected ${kase.expect.canonical_json}, got ${observed.canonicalJson}`,
      );
    }
  }
}

async function messageCase(api, kase) {
  const out = [];
  let bytes;
  let wire;
  if (kase.wire === 'json') {
    if (typeof kase.input !== 'string') return ['a json case without `input`'];
    bytes = new TextEncoder().encode(kase.input);
    wire = 'json';
  } else if (kase.wire === 'cbor') {
    // Never skipped, unlike the Rust run without its `cbor` feature: the component is
    // built with the codec, because ECMA-432 §7.1 is one of the two cases of
    // `types.wire` and a component that had only one would not implement the contract.
    if (typeof kase.input_hex !== 'string') return ['a cbor case without `input_hex`'];
    bytes = fromHex(kase.input_hex);
    wire = 'cbor';
  } else {
    return [`unknown wire \`${kase.wire}\``];
  }

  let decoded;
  let failure;
  try {
    decoded = await api.decode(bytes, wire);
  } catch (error) {
    if (error instanceof api.MalformedError) failure = error.error;
    // A decode that threw anything else is a failure of the *wrapper*: peer input must
    // not be able to produce one.
    else return [`decode threw ${String(error)}`];
  }

  if (kase.expect.decode === 'error') {
    if (failure === undefined) {
      out.push(`decode: expected an error, got a message (${compact(canonicalModel(decoded))})`);
    } else {
      compareDescription(kase, failure, out);
    }
  } else if (kase.expect.decode === 'ok') {
    if (failure !== undefined) {
      out.push(
        `decode: expected a message, got ${failure.kind} at ${failure.path}: ${failure.detail}`,
      );
    } else {
      compareObserved(kase, await observe(api, decoded), out);
    }
  } else {
    out.push(`unknown expect.decode \`${kase.expect.decode}\``);
  }
  return out;
}

// ---------------------------------------------------------------------------
// An exchange case
// ---------------------------------------------------------------------------

const DISPOSITIONS = ['reject', 'log', 'ignore'];

function policyOf(script, onError, onWarning) {
  const policy = {
    onError: script?.on_error ?? onError,
    onWarning: script?.on_warning ?? onWarning,
  };
  for (const value of [policy.onError, policy.onWarning]) {
    if (!DISPOSITIONS.includes(value)) {
      throw new Error(`unknown disposition \`${value}\`; expected reject, log or ignore`);
    }
  }
  return policy;
}

/**
 * The authority a case scripts, with the same three answers
 * `crates/conformance/src/exchange.rs`'s `ScriptedAuthority` gives.
 *
 * The three blocks are deliberately independent, and what joins them is one rule the
 * corpus states: *a value returned by `mint` joins `own` and `issued` from that moment,
 * because from that moment the end-point did create it.* Everything else is a lookup.
 */
function scriptedAuthority(script) {
  const own = new Set(script?.own ?? []);
  const issued = { ...(script?.issued ?? {}) };
  const mintable = { ...(script?.mint ?? {}) };
  const handedOut = {};
  return {
    isOwn: (token) => own.has(token) || Object.values(handedOut).includes(token),
    issued: (kind) => handedOut[kind] ?? issued[kind],
    mint: (kind) => {
      const fresh = mintable[kind];
      if (fresh === undefined) return undefined;
      handedOut[kind] = fresh;
      return fresh;
    },
  };
}

function tagged(direction, findings) {
  return findings.map((finding) => ({
    direction,
    rule: finding.violation.rule,
    severity: finding.violation.severity,
    path: finding.violation.path,
  }));
}

function compareRefusal(kase, refusal, out) {
  const expected = kase.expect.refused;
  if (expected === undefined) {
    if (kase.expect.outcome === 'refused') {
      out.push('expect.outcome is `refused` with no `refused` object');
    }
    return;
  }
  if (refusal.kind !== expected.kind) {
    out.push(`refused.kind: expected \`${expected.kind}\`, got \`${refusal.kind}\``);
  }
  if (expected.rules !== undefined) {
    const wanted = [...expected.rules].sort();
    const actual = [
      ...(refusal.kind === 'violations' ? refusal.violations.map((v) => v.rule) : []),
    ].sort();
    if (!same(wanted, actual)) {
      out.push(`refused.rules: expected ${compact(wanted)}, got ${compact(actual)}`);
    }
  }
  if (expected.count !== undefined && refusal.kind === 'too-many-tokens') {
    if (expected.count !== refusal.count) {
      out.push(`refused.count: expected ${expected.count}, got ${refusal.count}`);
    }
  }
  if (expected.limit !== undefined && refusal.kind === 'too-many-tokens') {
    if (expected.limit !== refusal.limit) {
      out.push(`refused.limit: expected ${expected.limit}, got ${refusal.limit}`);
    }
  }
}

function tokenKey(token) {
  const label = token.label === undefined ? '' : ` label=${token.label}`;
  return `${token.format}/${token.subformat} = ${compact(canonicalContent(token.content))}${label}`;
}

/**
 * §6.2's echo, compared as a **multiset**.
 *
 * The clause says what must come back and says nothing about where in the reply it goes,
 * so asserting the order would fail a conforming implementation for a choice the
 * standard leaves it. Matched by removal rather than by sorting, because an expectation
 * may be silent about a token's `label` and a sorted comparison cannot express "any".
 */
function compareTokens(wanted, reply, out) {
  const unmatched = (reply.submessages ?? []).filter((one) => one.format === 'token');
  for (const expected of wanted) {
    const at = unmatched.findIndex(
      (token) =>
        token.format === expected.format &&
        token.subformat === expected.subformat &&
        same(canonicalContent(token.content), expected.content) &&
        // An absent `label` key asserts nothing about one; an explicit `null` asserts
        // there is none.
        (!('label' in expected) ||
          (expected.label === null ? token.label === undefined : token.label === expected.label)),
    );
    if (at === -1) {
      const label =
        'label' in expected ? ` label=${expected.label === null ? 'none' : expected.label}` : '';
      out.push(
        `reply.tokens: expected but not returned: ${expected.format}/${expected.subformat} = ${compact(expected.content)}${label}`,
      );
    } else {
      unmatched.splice(at, 1);
    }
  }
  for (const extra of unmatched) {
    out.push(`reply.tokens: returned but not expected: ${tokenKey(extra)}`);
  }
}

function compareReply(kase, reply, out) {
  const expected = kase.expect.reply;
  if (expected === undefined) {
    if (kase.expect.outcome === 'reply') {
      out.push('expect.outcome is `reply` with no `reply` object');
    }
    return;
  }

  if ('messagetype' in expected) {
    const wanted = expected.messagetype === null ? undefined : expected.messagetype;
    if (reply.messageType !== wanted) {
      out.push(`reply.messagetype: expected ${compact(wanted)}, got ${compact(reply.messageType)}`);
    }
  }
  if ('control' in expected) {
    const wanted = expected.control === null ? undefined : expected.control;
    if (reply.control !== wanted) {
      out.push(`reply.control: expected ${compact(wanted)}, got ${compact(reply.control)}`);
    }
  }
  if (expected.first_submessage !== undefined) {
    const { format, subformat } = expected.first_submessage;
    if (reply.format !== format || reply.subformat !== subformat) {
      out.push(
        `reply.first_submessage: expected ${format}/${subformat}, got ${reply.format}/${reply.subformat}`,
      );
    }
  }
  if (expected.tokens !== undefined) compareTokens(expected.tokens, reply, out);
  if (expected.contains !== undefined) {
    for (const part of expected.contains) {
      const present = (reply.submessages ?? []).some(
        (one) => one.format === part.format && one.subformat === part.subformat,
      );
      if (!present) {
        out.push(
          `reply.contains: no ${part.format}/${part.subformat} submessage in ${compact(canonicalModel(reply))}`,
        );
      }
    }
  }
  if (expected.canonical !== undefined) {
    const actual = canonicalModel(reply);
    if (!same(actual, expected.canonical)) {
      out.push(`reply.canonical: expected ${compact(expected.canonical)}, got ${compact(actual)}`);
    }
  }
}

function compareFindings(kase, actual, out) {
  if (kase.expect.findings === undefined) return;
  const key = (one) => `${one.direction} ${one.severity} ${one.rule} at ${one.path}`;
  const wanted = kase.expect.findings.map(key).sort();
  const got = actual.map(key).sort();
  if (!same(wanted, got)) {
    out.push(`findings: expected ${compact(wanted)}, got ${compact(got)}`);
  }
}

function compareWithheld(kase, withheld, out) {
  if (kase.expect.withheld === undefined) return;
  if (withheld !== kase.expect.withheld) {
    out.push(`withheld: expected ${kase.expect.withheld}, got ${withheld}`);
  }
}

/** The clause-6 configuration and policy a case scripts, over the engine's defaults. */
async function scriptOf(api, script) {
  const defaults = await api.defaultConfig();
  const fallback = await api.defaultRejection();
  return {
    config: {
      issueConversationToken:
        script.config?.issue_conversation_token ?? defaults.issueConversationToken,
      issueAuthenticationToken:
        script.config?.issue_authentication_token ?? defaults.issueAuthenticationToken,
      maxTokensPerMessage: script.config?.max_tokens_per_message ?? defaults.maxTokensPerMessage,
    },
    rejection: {
      inbound: policyOf(
        script.rejection?.inbound,
        fallback.inbound.onError,
        fallback.inbound.onWarning,
      ),
      outbound: policyOf(
        script.rejection?.outbound,
        fallback.outbound.onError,
        fallback.outbound.onWarning,
      ),
    },
  };
}

/**
 * One exchange case: `check`, then `finish`, as steps 3 and 4 of
 * `conformance/README.md`'s algorithm.
 *
 * Both steps run under the **same** `cx.exchange`, so the scripted authority is asked
 * about one exchange — which is what `context.exchange` of a non-zero value is for. The
 * end-point's `answer` is never reached, and is scripted to say so.
 */
async function exchangeCase(api, kase) {
  const out = [];
  const script = kase.endpoint ?? {};
  let request;
  try {
    request = await api.decode(new TextEncoder().encode(JSON.stringify(kase.request)), 'json');
  } catch (error) {
    return [`request does not decode: ${String(error)}`];
  }

  let scripted;
  try {
    scripted = await scriptOf(api, script);
  } catch (error) {
    return [String(error.message ?? error)];
  }

  const swallowed = [];
  const endpoint = await api.createEndpoint({
    name: 'corpus',
    config: scripted.config,
    rejection: scripted.rejection,
    authority: scriptedAuthority(script.authority),
    answer: () => {
      throw new Error('the corpus never asks an end-point to answer');
    },
    onHostError: (error, where) => swallowed.push(`${where}: ${String(error)}`),
  });

  // One exchange, named by this runner rather than minted per call, so that `check` and
  // `finish` are two steps of one — `crates/conformance/src/component.rs` does the same.
  const cx = { exchange: nextExchange() };
  try {
    const findings = [];
    let checked;
    try {
      checked = await api.corpus.check(request, cx);
    } catch (error) {
      if (!(error instanceof api.RefusedError)) throw error;
      if (kase.expect.outcome !== 'refused') {
        out.push(`outcome: expected \`${kase.expect.outcome}\`, the request was refused`);
      }
      compareRefusal(kase, error.rejected, out);
      compareFindings(kase, findings, out);
      return out;
    }

    if (kase.expect.outcome !== 'reply') {
      out.push(`outcome: expected \`${kase.expect.outcome}\`, the request was accepted`);
    }
    findings.push(...tagged('inbound', checked));

    if (script.reply === undefined) {
      out.push('expect.outcome is `reply` with no `endpoint.reply`');
      return out;
    }
    let reply;
    try {
      reply = await api.decode(new TextEncoder().encode(JSON.stringify(script.reply)), 'json');
    } catch (error) {
      out.push(`endpoint.reply does not decode: ${String(error)}`);
      return out;
    }

    const finished = await api.corpus.finish(request, reply, cx);
    findings.push(...tagged('outbound', finished.findings));
    compareReply(kase, finished.message, out);
    compareWithheld(kase, finished.withheld, out);
    compareFindings(kase, findings, out);
    if (swallowed.length > 0) out.push(`the host threw: ${swallowed.join('; ')}`);
    return out;
  } finally {
    endpoint.close();
  }
}

/**
 * One `short_circuit` exchange case: the composed order, with the reply supplied by **a
 * layer whose `before` answers**.
 *
 * This is the only route there is: the host's `answer`
 * returns a message and the finishing step is the engine's, so a reply that skipped it
 * can only come from a layer above it. `serveMessage` then runs clause 6's inbound step,
 * asks layer 0, gets `answer(reply)`, never reaches the handler or the finishing step,
 * and runs clause 6's idempotent half over the reply.
 *
 * What it reports is exactly what the case asserts: `served.findings` are the inbound
 * ones alone, because the outbound step never ran.
 */
async function shortCircuitCase(api, kase) {
  const out = [];
  const script = kase.endpoint ?? {};
  if (script.reply === undefined) {
    return ['a `short_circuit` case needs an `endpoint.reply`: it is what the layer answered'];
  }

  let request;
  let reply;
  try {
    request = await api.decode(new TextEncoder().encode(JSON.stringify(kase.request)), 'json');
    reply = await api.decode(new TextEncoder().encode(JSON.stringify(script.reply)), 'json');
  } catch (error) {
    return [`a case message does not decode: ${String(error)}`];
  }

  let scripted;
  try {
    scripted = await scriptOf(api, script);
  } catch (error) {
    return [String(error.message ?? error)];
  }

  const entered = [];
  const unwound = [];
  const swallowed = [];
  const endpoint = await api.createEndpoint({
    name: 'corpus',
    config: scripted.config,
    rejection: scripted.rejection,
    authority: scriptedAuthority(script.authority),
    // Scripted with nothing, so an implementation that reached the handler fails loudly
    // rather than answering from a script that happened to match.
    answer: () => {
      throw new Error('a short-circuited exchange must not reach the handler');
    },
    layers: [
      {
        name: 'short-circuit',
        before: () => {
          entered.push('before');
          return api.answer(reply);
        },
        after: (message) => {
          unwound.push('after');
          return message;
        },
      },
    ],
    onHostError: (error, where) => swallowed.push(`${where}: ${String(error)}`),
  });

  try {
    const served = await api.corpus.serveMessage(request, { exchange: nextExchange() });
    if (served.outcome.kind === 'ok') {
      if (kase.expect.outcome !== 'reply') {
        out.push(`outcome: expected \`${kase.expect.outcome}\`, the request was answered`);
      }
      compareReply(kase, served.reply, out);
      compareWithheld(kase, served.withheld, out);
      compareFindings(kase, tagged('inbound', served.findings), out);
    } else if (served.outcome.kind === 'refused') {
      if (kase.expect.outcome !== 'refused') {
        out.push(`outcome: expected \`${kase.expect.outcome}\`, the request was refused`);
      }
      compareRefusal(kase, served.outcome.rejected, out);
      compareFindings(kase, [], out);
    } else {
      out.push(
        `outcome: \`serveMessage\` answered ${compact(served.outcome)}, which no ` +
          '`short_circuit` case expects',
      );
    }

    // The properties the case shape exists to record.
    if (entered.length === 0) out.push("the layer's `before` was never called");
    if (unwound.length > 0) {
      out.push("a short-circuited exchange unwound through the answering layer's own `after`");
    }
    if (swallowed.length > 0) out.push(`the host threw: ${swallowed.join('; ')}`);
    return out;
  } finally {
    endpoint.close();
  }
}

/**
 * One exchange number per case, so that a case driving `check` and `finish` drives them
 * as two steps of **one** exchange.
 *
 * `1` upward: any non-zero value is the caller's and travels unchanged, and the guest's
 * own counter is not something a host can collide with here, because a number a host
 * supplies is never minted (`wit/nlipsy.wit`, `types.context`).
 */
let exchangeCounter = 0n;
function nextExchange() {
  exchangeCounter += 1n;
  return exchangeCounter;
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

/**
 * Run every case against `api`.
 *
 * `api` is this package's public surface — the module object of `nlipsy` — and nothing
 * else: the runner reaches for `decode`, `encode`, `validate`, `isControl`,
 * `extractText`, `tokenKindOf`, `createEndpoint`, `answer`, `corpus.check`,
 * `corpus.finish`, `corpus.serveMessage`, `MalformedError`, `RefusedError`,
 * `defaultConfig` and `defaultRejection`, which is the whole of what a conformance runner
 * in any host language would need.
 *
 * `cases` is `[{ id, case }]`, already parsed with {@link parseCase}.
 *
 * Every case runs even when an earlier one has failed, and each difference is one line
 * naming the file — a corpus that stopped at the first mismatch would turn a reading of
 * the standard into one bisection per case.
 */
export async function runCorpus(api, cases) {
  let passed = 0;
  let skipped = 0;
  const failed = [];
  const groups = new Map();

  for (const { id, case: kase } of cases) {
    const group = id.includes('/') ? id.slice(0, id.lastIndexOf('/')) : '(root)';
    if (!groups.has(group)) groups.set(group, { cases: 0, passed: 0, failed: 0 });
    const counts = groups.get(group);
    counts.cases += 1;

    let differences;
    try {
      if ((kase.kind ?? 'message') === 'exchange') {
        differences = kase.endpoint?.short_circuit
          ? await shortCircuitCase(api, kase)
          : await exchangeCase(api, kase);
      } else {
        differences = await messageCase(api, kase);
      }
    } catch (error) {
      differences = [`the runner threw: ${String(error && error.stack ? error.stack : error)}`];
    }

    if (differences.length === 0) {
      passed += 1;
      counts.passed += 1;
    } else {
      failed.push({ id, reason: differences.join('; ') });
      counts.failed += 1;
    }
  }

  return { total: cases.length, passed, skipped, failed, groups };
}

/** The report `crates/conformance` prints, in the same order: differences, then counts. */
export function format(report) {
  const lines = [];
  for (const { id, reason } of report.failed) lines.push(`${id}: ${reason}`);
  for (const [group, counts] of [...report.groups].sort(([a], [b]) => (a < b ? -1 : 1))) {
    lines.push(
      `conformance: ${group}: ${counts.passed} passed` +
        (counts.failed > 0 ? `, ${counts.failed} failed` : '') +
        `, of ${counts.cases}`,
    );
  }
  lines.push(
    `conformance: ${report.passed} passed, ${report.skipped} skipped, ${report.failed.length} failed, of ${report.total} cases`,
  );
  return lines.join('\n');
}
