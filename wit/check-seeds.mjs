/**
 * The seed rule, checked: every `path:line` in the contract still names its item.
 *
 * `README.md` §1 makes the seed a rule — "An item with no seed may not be added" — and
 * says why the doc comments are load-bearing: wit-bindgen copies them into the generated
 * Rust, so a line number written in `nlipsy.wit` ships to a Rust caller's editor, and the
 * npm package's `types.ts` restates them by hand because jco copies **none** of them into
 * its `.d.ts` (`README.md` §5, measured). A table that can be wrong without anything
 * noticing is a convention rather than a rule, and a `file:line` drifts the moment the
 * file above it grows. This is what stops that: `make check-wit` runs it, it needs Node
 * and nothing else, and it reads the same two files a reader does.
 *
 * # The citation format it reads
 *
 * A **citation** is a backticked location: `crates/nlipsy-core/src/serve.rs:263`, a path
 * relative to the repository root and a line. Two short spellings are allowed inside one
 * block: `:201` means the same file as the citation before it, and `message.rs:84` means
 * the same file spelled short. A range — `serve.rs:373–374` — is one citation over
 * several lines.
 *
 * A **block** is the text a citation is read in the company of: one doc comment in
 * `nlipsy.wit`, one line — a table row — in §3 of `README.md`. Every backticked Rust name
 * in the block is a candidate for what the citation is *for*, and so is the WIT item the
 * block belongs to, in the three spellings Rust uses for a kebab-case WIT name
 * (`status-map` → `StatusMap`, `status_map`, `STATUS_MAP`). A qualified name contributes
 * its parts: `Conformity::check` is checked as `check` and as `Conformity`, because a
 * citation may point at the method or at the type that holds it.
 *
 * The check is that the cited line mentions one of them, as a word. That is what drift
 * breaks — a definition that moves leaves the old line saying something else — and it is
 * what a reader following the citation is looking for. A citation with no candidate name
 * anywhere is a failure too: there is nothing to check it against, which is the same hole
 * by another route.
 *
 * Exit code 0 and one line of counts, or the list of failures and exit code 1.
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');

/** A backticked `path:line`, `path:from–to`, `file.rs:line` or `:line`. */
const CITATION = /`((?:[A-Za-z0-9_./-]*\.rs)?):(\d+)(?:[–-](\d+))?`/g;
/** Anything backticked, which is where both the names and the citations live. */
const BACKTICKED = /`([^`]+)`/g;

/** Every cited file, read once. */
const files = new Map();

function linesOf(file) {
  if (!files.has(file)) {
    const at = path.join(root, file);
    files.set(file, fs.existsSync(at) ? fs.readFileSync(at, 'utf8').split('\n') : undefined);
  }
  return files.get(file);
}

/**
 * The words a backticked token contributes, or none if it is not a Rust name.
 *
 * WIT names are backticked in these files too — `owed-by`, `parse-error.detail` — and a
 * hyphen or a dot is what tells them apart. `impl Default for StatusMap` is the one
 * multi-word spelling the tables use, and the item in it is the last word.
 */
function wordsOf(token) {
  const one = token.trim();
  if (/^impl\s+.*\s+for\s+[A-Za-z_][A-Za-z0-9_]*$/.test(one)) return [one.split(/\s+/).pop()];
  if (!/^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*$/.test(one)) return [];
  // Both halves of a qualified name: a citation may point at the method or at the type.
  return one.split('::');
}

/** The Rust spellings of a WIT name, for a block whose prose names no Rust item. */
function spellingsOf(witName) {
  const parts = witName.replace(/^%/, '').split(/[-.]/).filter((part) => part !== '');
  if (parts.length === 0) return [];
  const pascal = parts.map((part) => part[0].toUpperCase() + part.slice(1)).join('');
  return [pascal, parts.join('_'), parts.join('_').toUpperCase(), parts[parts.length - 1]];
}

/** How Rust-ish a name looks, so that a failure message names what a reader would mean. */
function rank(name) {
  if (/^[A-Z][A-Z0-9_]*$/.test(name)) return 3;
  if (/^[A-Z]/.test(name) || name.includes('_')) return 2;
  return 1;
}

/** Whether a line mentions a name, as a word. */
function mentions(text, name) {
  return new RegExp(`\\b${name}\\b`).test(text);
}

/**
 * Every citation in one block, checked.
 *
 * `where` names the file and the line the citation is *written* on, which is what a
 * reader has to edit; `item` is the WIT item the block documents, when there is one.
 */
function checkBlock(block, where, item, failures) {
  const names = [];
  BACKTICKED.lastIndex = 0;
  for (let found = BACKTICKED.exec(block); found !== null; found = BACKTICKED.exec(block)) {
    for (const word of wordsOf(found[1])) names.push({ word, at: found.index });
  }
  const fallback = item === undefined ? [] : spellingsOf(item);

  let checked = 0;
  let lastPath;
  CITATION.lastIndex = 0;
  for (let found = CITATION.exec(block); found !== null; found = CITATION.exec(block)) {
    const [cited, spelt, from, to] = found;
    let file = spelt;
    if (file === '' || !file.includes('/')) {
      if (lastPath === undefined || (file !== '' && path.basename(lastPath) !== file)) {
        failures.push(`${where}: ${cited} is a short spelling with no full path before it`);
        continue;
      }
      file = lastPath;
    }
    lastPath = file;
    checked += 1;

    const text = linesOf(file);
    if (text === undefined) {
      failures.push(`${where}: ${cited} — no such file`);
      continue;
    }
    const first = Number(from);
    const last = to === undefined ? first : Number(to);
    if (first < 1 || last > text.length) {
      failures.push(`${where}: ${cited} — ${file} has ${text.length} lines`);
      continue;
    }

    const candidates = [...names.map((one) => one.word), ...fallback];
    if (candidates.length === 0) {
      failures.push(`${where}: ${cited} names no item, so nothing checks it`);
      continue;
    }
    const cut = text.slice(first - 1, last).join('\n');
    if (candidates.some((name) => mentions(cut, name))) continue;
    // The nearest name before the citation, of the most Rust-ish kind, is what a reader
    // would have meant by it — so that is what the failure names.
    const nearest = names
      .filter((one) => one.at < found.index)
      .sort((a, b) => rank(b.word) - rank(a.word) || b.at - a.at)[0];
    const meant = nearest?.word ?? fallback[0];
    failures.push(
      `${where}: ${cited} does not mention \`${meant}\`\n` +
        `    ${file}:${first} is: ${text[first - 1].trim()}`,
    );
  }
  return checked;
}

/**
 * `nlipsy.wit`, one doc comment at a time.
 *
 * The item a doc comment is for is the first thing on the line under it, which is how the
 * fallback spelling finds `status-map` for the doc above `record status-map`.
 */
function wit(failures) {
  const text = fs.readFileSync(path.join(here, 'nlipsy.wit'), 'utf8').split('\n');
  let checked = 0;
  let block = [];
  let start = 0;
  for (const [index, line] of text.entries()) {
    if (/^\s*\/\/\//.test(line)) {
      if (block.length === 0) start = index + 1;
      block.push(line);
      continue;
    }
    if (block.length > 0) {
      const declared = /^\s*(?:[a-z-]+\s+)?([%A-Za-z][A-Za-z0-9_.-]*)/.exec(line);
      checked += checkBlock(block.join('\n'), `wit/nlipsy.wit:${start}`, declared?.[1], failures);
      block = [];
    }
  }
  if (block.length > 0) {
    checked += checkBlock(block.join('\n'), `wit/nlipsy.wit:${start}`, undefined, failures);
  }
  return checked;
}

/**
 * `README.md` §3, one line at a time — a table row is a block.
 *
 * The item is the row's first cell, which is the WIT name the seed is for.
 */
function readme(failures) {
  const text = fs.readFileSync(path.join(here, 'README.md'), 'utf8').split('\n');
  const from = text.findIndex((line) => line.startsWith('## 3.'));
  const to = text.findIndex((line, index) => index > from && line.startsWith('## 4.'));
  if (from < 0 || to < 0) {
    failures.push('wit/README.md: §3 and §4 are what this reads between, and one is missing');
    return 0;
  }
  let checked = 0;
  for (let index = from; index < to; index += 1) {
    const row = text[index];
    const first = /^\|\s*`([^`]+)`/.exec(row);
    checked += checkBlock(row, `wit/README.md:${index + 1}`, first?.[1], failures);
  }
  return checked;
}

const failures = [];
const counted = wit(failures) + readme(failures);
if (failures.length > 0) {
  process.stderr.write(`${failures.join('\n')}\n`);
  process.stderr.write(`\n${failures.length} of ${counted} seed citations are wrong\n`);
  process.exit(1);
}
process.stdout.write(`${counted} seed citations name the item they point at\n`);
