/**
 * The seed rule, checked: every citation in the contract names an item that is defined
 * where it says.
 *
 * `README.md` §1 makes the seed a rule — "An item with no seed may not be added" — and
 * says why the doc comments are load-bearing: wit-bindgen copies them into the generated
 * Rust, so a citation written in `nlipsy.wit` ships to a Rust caller's editor, and the
 * npm package's `types.ts` restates them by hand because jco copies **none** of them into
 * its `.d.ts` (`README.md` §5, measured). A table that can be wrong without anything
 * noticing is a convention rather than a rule. This is what stops that: `make check-wit`
 * runs it, it needs Node and nothing else, and it reads the same two files a reader does.
 *
 * # The citation format
 *
 * A **citation** is one backticked token: a path relative to the repository root, `::`,
 * and one or two Rust path segments —
 *
 *     `crates/nlipsy-core/src/serve.rs::serve`
 *     `crates/nlipsy-server/src/server.rs::ServerConfig::principal_header`
 *
 * It names an item and never a line. A line number drifts with every edit above it, and
 * it drifts silently: the line it lands on usually still mentions *something* nearby. An
 * item moves only when somebody moves it, and then this check fails on every citation
 * that has to follow it — naming where the item now is, when it is still in that file.
 *
 * Every backticked token of that shape in `nlipsy.wit` and in `README.md` is a citation,
 * wherever it appears. A backticked `file.rs` followed by a colon and a line number is a
 * failure rather than something to read: that form is retired, and a copy of it pasted
 * back in is caught here rather than left to drift.
 *
 * # What "defined where it says" means
 *
 * **`path::NAME`** is a free item: a line that declares `fn`, `struct`, `enum`, `trait`,
 * `type`, `const`, `static`, `mod` or `union` NAME — after any of `pub`, `pub(…)`,
 * `const`, `async`, `unsafe`, `extern "…"` or `default` — or `macro_rules! NAME`, and
 * that is not inside an `impl` or `trait` body. A method is not a free item; it is cited
 * with its owner.
 *
 * **`path::OWNER::NAME`** is a member of OWNER, and one of:
 *
 * - an **associated item**: a `fn`, `const` or `type` NAME whose nearest enclosing item
 *   is an `impl` block for OWNER — inherent (`impl OWNER`, `impl<T> OWNER<T>`) or a
 *   trait's (`impl Default for OWNER`, which is how `StatusMap::default` is cited) — or
 *   `trait OWNER` itself, where a required or provided method is declared;
 * - a **field** `NAME:` of `struct OWNER` or `union OWNER`, or a **variant** NAME of
 *   `enum OWNER`, at the first level of that item's body.
 *
 * The file must contain the definition; OWNER's own definition may be elsewhere, because
 * an `impl` need not sit beside its type and the citation points at the `impl`.
 *
 * # How the source is read
 *
 * As rustfmt leaves it, which `cargo fmt --all --check` in `make check` guarantees: an
 * item's body opens with a `{` that ends a line at the item's own indentation, and
 * closes at the first later line that starts with `}` at that same indentation. Nesting
 * is read from that and nothing else — no Rust is parsed, and a macro invocation's braces
 * (`cfg_client! { … }`) are not an item, so what is declared inside one is read as if
 * the braces were not there.
 *
 * Exit code 0 and one line of counts, or the list of failures and exit code 1.
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');

const IDENT = '[A-Za-z_][A-Za-z0-9_]*';
/** Anything backticked that ends in `.rs::` and a path: shaped like a citation. */
const CITATION = new RegExp(`\`([^\`\\s]*\\.rs)((?:::${IDENT})+)\``, 'g');
/** The retired form: a backticked `.rs` path, or none, then a colon and a line or a range. */
const LINE_NUMBER = /`(?:[^`\s]*\.rs)?:\d+(?:[–-]\d+)?`/g;
/** A declaration, with whatever visibility and qualifiers precede its keyword. */
const DECLARATION = new RegExp(
  `^(\\s*)(?:pub(?:\\s*\\([^)]*\\))?\\s+)?` +
    `(?:(?:const|async|unsafe|default|extern(?:\\s+"[^"]*")?)\\s+)*` +
    `(fn|struct|enum|trait|type|const|static|mod|union)\\s+(?:r#)?(${IDENT})`,
);
const MACRO_RULES = new RegExp(`^(\\s*)macro_rules!\\s+(${IDENT})`);
const IMPL = /^(\s*)(?:unsafe\s+)?impl\b/;
/** The kinds whose body is a scope for the rule above; `const`, `static`, `type` have none. */
const SCOPES = new Set(['fn', 'struct', 'enum', 'trait', 'mod', 'union', 'impl']);

const indentOf = (line) => /^\s*/.exec(line)[0].length;

/** The line without a trailing `//` comment, for reading how it ends. */
function code(line) {
  const at = line.search(/\s\/\/(?!\/)/);
  if (at < 0) return line.trimEnd();
  const before = line.slice(0, at);
  // A `//` inside a string literal (a URL, say) is not a comment.
  return (before.match(/"/g) ?? []).length % 2 === 0 ? before.trimEnd() : line.trimEnd();
}

/**
 * The extent of an item's body: the line its `{` ends and the line its `}` starts, or
 * `undefined` for an item that has none (`fn f();`, `struct S;`, `const X: T = …;`).
 */
function bodyOf(lines, at, indent) {
  for (let index = at; index < lines.length; index += 1) {
    const text = code(lines[index]);
    if (index > at && text.trim() !== '' && indentOf(text) < indent) return undefined;
    if (index > at && indentOf(text) !== indent) continue;
    if (text.endsWith(';')) return undefined;
    if (index === at && /\{.*\}[;,]?$/.test(text)) return { open: index, close: index };
    if (!text.endsWith('{')) continue;
    for (let close = index + 1; close < lines.length; close += 1) {
      if (indentOf(lines[close]) === indent && lines[close].trim().startsWith('}')) {
        return { open: index, close };
      }
    }
    return undefined;
  }
  return undefined;
}

/** The type an `impl` header is for: `Conformity` in `impl<E, A> Conformity<E, A>`. */
function selfTypeOf(header) {
  let text = header.replace(/^\s*(?:unsafe\s+)?impl\s*/, '');
  if (text.startsWith('<')) {
    let depth = 0;
    for (let index = 0; index < text.length; index += 1) {
      if (text.startsWith('->', index)) index += 1;
      else if (text[index] === '<') depth += 1;
      else if (text[index] === '>' && --depth === 0) {
        text = text.slice(index + 1);
        break;
      }
    }
  }
  text = text.split(/\swhere\b|\{/)[0];
  const forAt = text.search(/\sfor\s/);
  if (forAt >= 0) text = text.slice(forAt).replace(/^\s+for\s+/, '');
  const found = new RegExp(`^[\\s&]*(?:'${IDENT}\\s+)?(?:(?:mut|dyn)\\s+)?(?:${IDENT}::)*(${IDENT})`).exec(text);
  return found?.[1];
}

/** Every item in one file — what it is, what it is called, where its body is. */
function itemsOf(lines) {
  const items = [];
  for (const [at, line] of lines.entries()) {
    const declared = DECLARATION.exec(line);
    const macro = MACRO_RULES.exec(line);
    const impl = IMPL.exec(line);
    let item;
    if (declared) item = { kind: declared[2], name: declared[3], indent: declared[1].length };
    else if (macro) item = { kind: 'macro_rules', name: macro[2], indent: macro[1].length };
    else if (impl) {
      // The header runs to its `{`, through a `where` clause if there is one.
      let header = '';
      for (let index = at; index < lines.length; index += 1) {
        header += ` ${code(lines[index])}`;
        if (code(lines[index]).endsWith('{')) break;
      }
      item = { kind: 'impl', name: selfTypeOf(header.trim()), indent: impl[1].length };
    } else continue;
    item.at = at;
    if (SCOPES.has(item.kind)) item.body = bodyOf(lines, at, item.indent);
    items.push(item);
  }
  // The nearest enclosing item of each, which is what decides free versus member.
  for (const item of items) {
    item.parent = items
      .filter((one) => one.body && one.body.open < item.at && item.at < one.body.close)
      .sort((a, b) => b.at - a.at)[0];
  }
  return items;
}

/** Every cited file, read once and indexed. */
const sources = new Map();

function sourceOf(file) {
  if (!sources.has(file)) {
    const at = path.join(root, file);
    if (!fs.existsSync(at)) sources.set(file, undefined);
    else {
      const lines = fs.readFileSync(at, 'utf8').split('\n');
      sources.set(file, { lines, items: itemsOf(lines) });
    }
  }
  return sources.get(file);
}

const isMember = (item) => item.parent !== undefined && ['impl', 'trait'].includes(item.parent.kind);

/** Whether `name` is defined in `source` as the citation's segments say. */
function defines(source, segments) {
  if (segments.length === 1) {
    const [name] = segments;
    return source.items.some((item) => item.name === name && item.kind !== 'impl' && !isMember(item));
  }
  const [owner, name] = segments;
  const associated = source.items.some(
    (item) =>
      item.name === name &&
      ['fn', 'const', 'type'].includes(item.kind) &&
      isMember(item) &&
      item.parent.name === owner,
  );
  if (associated) return true;
  // A field or a variant: a line one level inside `struct OWNER`, `union OWNER` or `enum OWNER`.
  const field = new RegExp(`^\\s*(?:pub(?:\\s*\\([^)]*\\))?\\s+)?${name}\\s*:`);
  const variant = new RegExp(`^\\s*${name}\\s*(?:[({,=]|$)`);
  return source.items.some((item) => {
    if (item.name !== owner || item.body === undefined) return false;
    if (!['struct', 'union', 'enum'].includes(item.kind)) return false;
    const shape = item.kind === 'enum' ? variant : field;
    return source.lines
      .slice(item.body.open + 1, item.body.close)
      .some((line) => indentOf(line) === item.indent + 4 && shape.test(code(line)));
  });
}

/** Where `name` *is* defined in the file, as citations, for a failure message. */
function spellingsIn(file, source, name) {
  return source.items
    .filter((item) => item.name === name && item.kind !== 'impl')
    .map((item) => (isMember(item) ? `${file}::${item.parent.name}::${name}` : `${file}::${name}`));
}

/** Every citation in one file of the contract, checked. */
function check(relative, failures) {
  const lines = fs.readFileSync(path.join(root, relative), 'utf8').split('\n');
  let checked = 0;
  for (const [index, line] of lines.entries()) {
    const where = `${relative}:${index + 1}`;
    for (const found of line.matchAll(LINE_NUMBER)) {
      failures.push(`${where}: ${found[0]} cites a line; cite the item defined there instead`);
    }
    for (const found of line.matchAll(CITATION)) {
      const [cited, file, rest] = found;
      const segments = rest.slice(2).split('::');
      checked += 1;
      if (!file.includes('/') || file.startsWith('/') || file.startsWith('.')) {
        failures.push(`${where}: ${cited} — the path is not relative to the repository root`);
        continue;
      }
      if (segments.length > 2) {
        failures.push(`${where}: ${cited} — one or two segments after the path, not ${segments.length}`);
        continue;
      }
      const source = sourceOf(file);
      if (source === undefined) {
        failures.push(`${where}: ${cited} — no such file`);
        continue;
      }
      if (defines(source, segments)) continue;
      const elsewhere = spellingsIn(file, source, segments[segments.length - 1]);
      failures.push(
        `${where}: ${cited} — not defined there` +
          (elsewhere.length > 0 ? `; ${file} defines ${elsewhere.map((one) => `\`${one}\``).join(', ')}` : ''),
      );
    }
  }
  return checked;
}

const failures = [];
const counted = check('wit/nlipsy.wit', failures) + check('wit/README.md', failures);
if (failures.length > 0) {
  process.stderr.write(`${failures.join('\n')}\n`);
  process.stderr.write(`\n${failures.length} problems among ${counted} seed citations\n`);
  process.exit(1);
}
process.stdout.write(`${counted} seed citations name an item defined where they say\n`);
