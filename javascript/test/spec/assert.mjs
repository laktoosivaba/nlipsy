/**
 * The four assertions the shared specs use.
 *
 * Plain functions that throw, and no test framework in them: the specs in `test/spec/`
 * are named by `test/node/*.test.mjs` under vitest today and are the same modules a Bun
 * or a browser harness would name on the day either can run them. The harness files are
 * thin on purpose — they name the cases and let these throw.
 */

/** Fail with `why` unless `condition`. */
export function ok(condition, why) {
  if (!condition) throw new Error(why);
}

/** Structural equality, with `Uint8Array` compared by bytes and `bigint` kept apart. */
export function deepEqual(actual, expected, why) {
  if (!same(actual, expected)) {
    throw new Error(`${why}\n  expected ${show(expected)}\n  got      ${show(actual)}`);
  }
}

/** Fail unless `actual` is `expected`, by `Object.is`. */
export function equal(actual, expected, why) {
  if (!Object.is(actual, expected)) {
    throw new Error(`${why}: expected ${show(expected)}, got ${show(actual)}`);
  }
}

/** Run `body` and hand back what it threw, failing if it threw nothing. */
export async function throws(body, why) {
  try {
    await body();
  } catch (error) {
    return error;
  }
  throw new Error(`${why}: nothing was thrown`);
}

export function same(left, right) {
  if (Object.is(left, right)) return true;
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

export function show(value) {
  return JSON.stringify(
    value,
    (_key, one) => {
      if (typeof one === 'bigint') return `${one}n`;
      if (one instanceof Uint8Array) return `Uint8Array(${[...one].join(',')})`;
      return one;
    },
  );
}
