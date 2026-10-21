/**
 * The round trip: `Message` → the boundary's shape → `Message`, over every corpus
 * message.
 *
 * The corpus proves the *engine*. This proves the **wrapper's conversions**, and it uses
 * the corpus as its source of messages because those are the messages that exist: every
 * format, an empty subformat, a byte string, nested structured content, a token whose
 * content is an object, `u64::MAX`.
 *
 * The round trip is run through the engine rather than against `boundary.ts` directly,
 * and that is what makes it evidence: `encode(message)` lowers the public message across
 * the boundary and `decode` lifts it back, so a conversion that lost a field would come
 * back short. A test that called `toWitMessage` and `fromWitMessage` in sequence would
 * only prove the two agree with each other.
 */

import { deepEqual, equal, ok, throws } from './assert.mjs';
import { corpus } from './corpus.mjs';

/** Every corpus message that decodes, as a public `Message`. */
async function messages(api) {
  const out = [];
  for (const { id, case: kase } of corpus()) {
    if ((kase.kind ?? 'message') !== 'message') continue;
    if (kase.expect.decode !== 'ok') continue;
    const bytes =
      kase.wire === 'json'
        ? new TextEncoder().encode(kase.input)
        : Uint8Array.from(kase.input_hex.match(/../g).map((byte) => Number.parseInt(byte, 16)));
    out.push({ id, wire: kase.wire, message: await api.decode(bytes, kase.wire) });
  }
  return out;
}

export const specs = [
  {
    name: 'every corpus message survives the round trip through the boundary',
    async run(api) {
      const all = await messages(api);
      ok(all.length > 60, `expected the corpus's decoding messages, found ${all.length}`);
      for (const { id, wire, message } of all) {
        // Each message on **its own** wire. Not a convenience: JSON spells a `binary`
        // content as base64 (§6.4), so a message whose content arrived as a *text
        // string* under a `binary` format — which ECMA-432 §7.2 allows and
        // `ecma-432/06-cbor-binary-content-as-text-string` records — comes back from the
        // JSON wire as bytes. That is the model's asymmetry and not this boundary's; the
        // case below pins it, and this loop asks the question the wires can answer.
        const back = await api.decode(await api.encode(message, wire), wire);
        deepEqual(back, message, `${id}: the message did not come back as it went out`);
      }
      return `${all.length} messages`;
    },
  },

  {
    name: 'a binary content that arrived as text becomes bytes on the JSON wire',
    async run(api) {
      // ECMA-432 §7.2 lets a CBOR frame carry a `binary` content as a text string, and
      // §6.4 gives JSON exactly one spelling for a `binary` content: base64. So the two
      // wires disagree about this one message, and re-encoding it on the other one is
      // lossy in a way nothing in the wrapper could prevent. Recorded here so that a
      // reader who meets it finds the reason rather than a bug report.
      const asText = { format: 'binary', subformat: 'image/jpeg', content: '/9j/4A==' };
      const throughJson = await api.decode(await api.encode(asText, 'json'), 'json');
      ok(
        throughJson.content instanceof Uint8Array,
        `the JSON wire reads a binary content as base64: ${String(throughJson.content)}`,
      );
      deepEqual(
        throughJson.content,
        new Uint8Array([0xff, 0xd8, 0xff, 0xe0]),
        'and it is the base64 decoded',
      );
      // On its own wire it is unchanged.
      const throughCbor = await api.decode(await api.encode(asText, 'cbor'), 'cbor');
      deepEqual(throughCbor, asText, 'the CBOR wire carries it as it arrived');
    },
  },

  {
    name: 'a byte string stays a byte string and is not a base64 lookalike',
    async run(api) {
      const message = {
        format: 'binary',
        subformat: 'image/jpeg',
        content: new Uint8Array([0xff, 0xd8, 0xff, 0x00, 0x01]),
      };
      const back = await api.decode(await api.encode(message), 'json');
      ok(back.content instanceof Uint8Array, `content came back as ${typeof back.content}`);
      deepEqual(back.content, message.content, 'the bytes changed');

      // And the same bytes written as a string are a string: the two are different
      // values and JSON spells them identically, which is what §6.4's base64 costs and
      // what the corpus's `{"$bytes": …}` marker exists to keep apart on this side.
      const asText = { format: 'text', subformat: 'english', content: '/9j/AAE=' };
      const backText = await api.decode(await api.encode(asText), 'json');
      equal(typeof backText.content, 'string', 'a text content came back as something else');
    },
  },

  {
    name: 'an integer wider than a double crosses as a bigint, exactly',
    async run(api) {
      const huge = 18446744073709551615n; // u64::MAX — 2 048 from the nearest double
      const message = { format: 'structured', subformat: 'json', content: huge };
      const encoded = new TextDecoder().decode(await api.encode(message));
      equal(
        encoded,
        '{"format":"structured","subformat":"json","content":18446744073709551615}',
        'the widest u64 did not survive encoding',
      );
      const back = await api.decode(await api.encode(message), 'json');
      equal(typeof back.content, 'bigint', 'a u64 came back as something other than a bigint');
      equal(back.content, huge, 'the value moved');
    },
  },

  {
    name: 'an integer a double holds exactly comes back as a number, and zero is not absent',
    async run(api) {
      for (const value of [0, -1, 42, Number.MAX_SAFE_INTEGER, -Number.MAX_SAFE_INTEGER]) {
        const back = await api.decode(
          await api.encode({ format: 'structured', subformat: 'json', content: value }),
          'json',
        );
        equal(typeof back.content, 'number', `${value} came back as ${typeof back.content}`);
        equal(back.content, value, `${value} moved`);
      }
      // And a bigint inside the safe range narrows to a number, because that is what the
      // engine's own `as_i64` answer lifts to.
      const back = await api.decode(
        await api.encode({ format: 'structured', subformat: 'json', content: 7n }),
        'json',
      );
      equal(back.content, 7, 'a small bigint did not narrow');
    },
  },

  {
    name: 'a non-finite number is refused rather than invented into something else',
    async run(api) {
      for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
        const error = await throws(
          () => api.encode({ format: 'structured', subformat: 'json', content: value }),
          `${value} should not be encodable`,
        );
        ok(error instanceof TypeError, `expected a TypeError, got ${String(error)}`);
      }
    },
  },

  {
    name: 'an absent content is refused at the lowering, by name',
    async run(api) {
      // `undefined` is not a value ECMA-430 §5.1.4 can carry, and lowering it would
      // raise jco's own complaint about a variant it could not read — which names
      // neither this package nor the field.
      const error = await throws(
        () => api.encode({ format: 'text', subformat: 'english', content: undefined }),
        'an absent content should not be encodable',
      );
      ok(error instanceof TypeError, `expected a TypeError, got ${String(error)}`);
      ok(
        String(error.message).includes('undefined'),
        `the message should name the type it refused: ${error.message}`,
      );

      // The other shape of the same fact: an object with no JSON text at all.
      const nothing = await throws(
        () =>
          api.encode({
            format: 'structured',
            subformat: 'json',
            content: { toJSON: () => undefined },
          }),
        'content whose `toJSON` answers nothing should not be encodable',
      );
      ok(
        String(nothing.message).includes('nlipsy'),
        `the message should name this package: ${nothing.message}`,
      );
    },
  },

  {
    name: 'structured content crosses as JSON and comes back with its keys sorted',
    async run(api) {
      const message = {
        format: 'structured',
        subformat: 'json',
        content: { zebra: 1, apple: [true, null, 'x'], middle: { b: 2, a: 1 } },
      };
      const back = await api.decode(await api.encode(message), 'json');
      deepEqual(back.content, message.content, 'the structure changed');
      // The order is the engine's — a `BTreeMap`, and ECMA-404 objects are unordered —
      // and it is documented rather than asserted as a rule, because a reader who sees
      // their keys move should find the reason written down.
      deepEqual(
        Object.keys(back.content),
        ['apple', 'middle', 'zebra'],
        'structured keys came back in some order other than sorted',
      );
    },
  },

  {
    name: 'a byte string nested in structured content is base64, as JSON has no other spelling',
    async run(api) {
      const message = {
        format: 'structured',
        subformat: 'json',
        content: { blob: new Uint8Array([1, 2, 3]) },
      };
      const back = await api.decode(await api.encode(message), 'json');
      // The loss is JSON's and not this boundary's — and it is
      // base64 rather than `{"0":1,…}`, which would be a corruption.
      equal(back.content.blob, 'AQID', 'nested bytes were not base64');
    },
  },

  {
    name: 'an integer nested in structured content survives the round trip exactly',
    async run(api) {
      // The top-level case above is `ecma-432/12`'s and the corpus covers it. One level
      // down the corpus has no case, and `JSON.parse` and `JSON.stringify` both round —
      // so this is the test. The engine carries the value exactly on both
      // sides of this trip, so a difference here is the boundary's.
      const document =
        '{"format":"structured","subformat":"json","content":' +
        '{"float":1.5,"id":18446744073709551615,"small":7}}';
      const back = await api.decode(new TextEncoder().encode(document), 'json');
      equal(back.content.id, 18446744073709551615n, 'a nested u64::MAX was not kept');
      // And only where it is needed: a number a `number` holds stays one, and a float
      // stays a float, because a `bigint` everywhere would be a different lie.
      equal(back.content.small, 7, 'a small integer became something else');
      equal(back.content.float, 1.5, 'a float became something else');

      equal(
        new TextDecoder().decode(await api.encode(back, 'json')),
        document,
        'the document did not come back byte for byte',
      );
    },
  },

  {
    name: 'a bigint nested in structured content goes out as digits',
    async run(api) {
      // The other direction of the same fact. `JSON.stringify` refuses a `bigint` with
      // `TypeError: Do not know how to serialize a BigInt`, which names neither this
      // package nor the field; the replacer hands it to `JSON.rawJSON` instead, so the
      // digits reach the engine and the engine decides what kind of number they are.
      const message = {
        format: 'structured',
        subformat: 'json',
        content: { id: 18446744073709551615n, negative: -9007199254740993n },
      };
      equal(
        new TextDecoder().decode(await api.encode(message, 'json')),
        '{"format":"structured","subformat":"json","content":' +
          '{"id":18446744073709551615,"negative":-9007199254740993}}',
        'a nested bigint was not written as digits',
      );
      const back = await api.decode(await api.encode(message, 'json'), 'json');
      equal(back.content.id, 18446744073709551615n, 'and it did not come back');
      equal(back.content.negative, -9007199254740993n, 'the negative one did not come back');
    },
  },

  {
    name: "the nested base64 is the engine's own spelling",
    async run(api) {
      // `toBase64` is the one rule this wrapper holds and cannot delegate: WIT has no
      // recursive types, so the `structured` string is written in TypeScript and a byte
      // string inside it has to be spelled before it crosses. What stops the two sides
      // drifting is this — the expectation is not a literal a test author wrote but the
      // base64 the **engine** writes for the same bytes, read out of an encoded document.
      const cases = [
        new Uint8Array([1]),
        new Uint8Array([1, 2]),
        new Uint8Array([1, 2, 3]),
        new Uint8Array([0xff, 0x80, 0x7f, 0x00]),
      ];
      for (const bytes of cases) {
        const encoded = JSON.parse(
          new TextDecoder().decode(
            await api.encode({ format: 'binary', subformat: 'x/y', content: bytes }, 'json'),
          ),
        );
        const nested = new TextDecoder().decode(
          await api.encode(
            { format: 'structured', subformat: 'json', content: { blob: bytes } },
            'json',
          ),
        );
        equal(
          JSON.parse(nested).content.blob,
          encoded.content,
          `the nested base64 for [${[...bytes].join(',')}] is not the engine's`,
        );
      }
      return `${cases.length} byte strings`;
    },
  },

  {
    name: 'an absent optional is an absent key and never a null',
    async run(api) {
      const back = await api.decode(
        new TextEncoder().encode('{"format":"text","subformat":"english","content":null}'),
        'json',
      );
      equal(back.content, null, 'an explicit null content was lost');
      ok(!('messageType' in back), 'an absent messagetype came back as a key');
      ok(!('control' in back), 'an absent control came back as a key');
      ok(!('label' in back), 'an absent label came back as a key');
      ok(!('submessages' in back), 'an empty submessages list came back as a key');
    },
  },

  {
    name: 'a decode failure is a value on the error and not prose',
    async run(api) {
      const error = await throws(
        () => api.decode(new TextEncoder().encode('nope'), 'json'),
        'bytes that are not JSON should not decode',
      );
      ok(error instanceof api.MalformedError, `expected a MalformedError, got ${String(error)}`);
      equal(error.error.kind, 'invalid-json', 'the kind the corpus compares');
      equal(error.error.path, '$', 'the path the corpus compares');
      equal(error.code, 'malformed', 'the stable code a caller branches on');
    },
  },
];
