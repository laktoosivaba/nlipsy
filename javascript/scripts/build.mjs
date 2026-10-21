// Build the package: the component, jco once, then `tsc`.
//
// The order is not a preference. jco's output is what `src/*.ts` is typed against —
// `src/wasm/nlipsy.d.ts` and `src/wasm/interfaces/*.d.ts` are the only description of the
// boundary there is — so the transpile has to happen before the compile.
//
// It lands in `src/wasm/` and is copied to `dist/wasm/` afterwards, and that is worth a
// sentence because it looks like a detour. The specifier a module writes for the glue has
// to resolve in *both* trees: `src/glue.ts` writes `./wasm/nlipsy.js` so that `tsc` finds
// the declarations, and the emitted `dist/glue.js` writes the same string, which has to
// find the glue beside it. The same holds for the five `--map` targets, which are written
// into the generated glue as import specifiers and resolved at run time relative to the
// file that holds them: `../host/endpoint.js` from `dist/wasm/nlipsy.js` is
// `dist/host/endpoint.js`, which is what `tsc` emits from `src/host/endpoint.ts`.
// One spelling, two trees, no build-time rewriting.
//
// One transpile and not two: the package has one entry point, so the tarball carries one
// copy of the glue beside one `.wasm` and there is nothing to keep in step.

import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const pkg = path.resolve(here, '..');
const workspace = path.resolve(pkg, '..');

const DEFAULT_COMPONENT = path.join(
  workspace,
  'target/wasm32-wasip3/component/nlipsy_component.wasm',
);

/**
 * The five imports, by the name jco expects **without a version**.
 *
 * `--map` names an interface and not a versioned instance, which is why a bump of
 * `nlipsy:engine@0.1.0` does not reach these strings and why `wit/README.md` §6 says so
 * in its procedure. Each target is relative to the *output directory*, which is jco's
 * rule and the trap behind it: a map to `./host.js` is looked for beside the glue, not
 * beside the caller.
 */
const MAPS = [
  'nlipsy:engine/host-endpoint=../host/endpoint.js',
  'nlipsy:engine/host-layers=../host/layers.js',
  'nlipsy:engine/host-authority=../host/authority.js',
  'nlipsy:engine/host-transport=../host/transport.js',
  'nlipsy:engine/host-record-sink=../host/record-sink.js',
];

/**
 * The toolchain the artefact is built with, when this script has to build it.
 *
 * The same name and the same default as the `Makefile`'s `COMPONENT_TOOLCHAIN`, read
 * from the environment so that `make check-js COMPONENT_TOOLCHAIN=nightly` reaches here
 * too — make exports nothing by itself, so `check-js` passing it on is what joins them.
 * `wasm32-wasip3` has no `rust-std` on stable until Rust 1.100 (2026-11-12); on that day
 * this becomes `stable` and then goes.
 */
const TOOLCHAIN = process.env.COMPONENT_TOOLCHAIN || 'nightly-2026-09-15';

/** Run a command, inheriting stdio, and stop the build if it fails. */
function run(command, args, options = {}) {
  process.stdout.write(`$ ${command} ${args.join(' ')}\n`);
  const done = spawnSync(command, args, { stdio: 'inherit', cwd: pkg, ...options });
  if (done.status !== 0) {
    throw new Error(`${command} exited with ${done.status ?? done.signal}`);
  }
}

/**
 * Where the component is.
 *
 * `NLIPSY_COMPONENT` first, as `crates/conformance/src/component.rs` reads it, so that
 * one environment variable points every runner at the same artefact. Absent, the
 * `Makefile`'s own path — and if nothing is there, cargo builds it with the same command
 * `make component` runs: a package that failed with "no such file" would be telling a
 * contributor to go and read the Makefile.
 */
function component() {
  const named = process.env.NLIPSY_COMPONENT;
  if (named) {
    if (!fs.existsSync(named)) {
      throw new Error(`NLIPSY_COMPONENT names ${named}, which does not exist`);
    }
    return named;
  }
  if (!fs.existsSync(DEFAULT_COMPONENT)) {
    process.stdout.write('no component yet; building it\n');
    run('make', ['component', `COMPONENT_TOOLCHAIN=${TOOLCHAIN}`], { cwd: workspace });
  }
  return DEFAULT_COMPONENT;
}

const require = createRequire(import.meta.url);
// The CLI entry, beside the API entry the package's `exports` do name. Neither the
// manifest nor the command is reachable through `exports`, and `node_modules/.bin` is a
// shell shim rather than a module — so this resolves what *is* exported and steps across.
const jco = path.join(path.dirname(require.resolve('@bytecodealliance/jco')), 'jco.js');
if (!fs.existsSync(jco)) {
  throw new Error(`jco's CLI is not at ${jco}; the package's layout has moved`);
}

/**
 * The transpile.
 *
 * No `-O`/`-m`: there is no size budget, and an unminified glue is what a stack trace out
 * of the boundary points into. `--import-bindings optimized` is not available at all: it
 * panics on this wasip3 component (`unexpected non-host resource table`, jco 1.39.0).
 */
function transpile(artefact, out) {
  fs.rmSync(out, { recursive: true, force: true });
  run(process.execPath, [
    jco,
    'transpile',
    artefact,
    '-o',
    out,
    '--name',
    'nlipsy',
    ...MAPS.flatMap((one) => ['--map', one]),
  ]);
}

/** Does jco's output parse? A question worth asking, and cheap. */
function parseCheck(out) {
  const emitted = path.join(out, 'nlipsy.js');
  const done = spawnSync(process.execPath, ['--check', emitted], { stdio: 'inherit' });
  if (done.status !== 0) {
    throw new Error(`jco's output does not parse: ${path.relative(pkg, emitted)}`);
  }
}

/** Copy a directory, without depending on a Node version's `cp` flags. */
function copy(from, to) {
  fs.rmSync(to, { recursive: true, force: true });
  fs.cpSync(from, to, { recursive: true });
}

function main() {
  const artefact = component();
  process.stdout.write(`component: ${artefact}\n`);

  const wasm = path.join(pkg, 'src/wasm');
  transpile(artefact, wasm);
  parseCheck(wasm);

  // From scratch, because `tsc` only overwrites what it emits: a module deleted from
  // `src/`, or a declaration a previous build left behind, would otherwise stay in the
  // tarball. `npm pack --dry-run` is the check that catches it, and this is the fix.
  fs.rmSync(path.join(pkg, 'dist'), { recursive: true, force: true });

  // The wrapper.
  run(process.execPath, [require.resolve('typescript/bin/tsc'), '-p', 'tsconfig.json']);

  // jco's output is not compiled, so `tsc` does not carry it across.
  copy(wasm, path.join(pkg, 'dist/wasm'));

  const glue = fs.statSync(path.join(pkg, 'dist/wasm/nlipsy.js')).size;
  const core = fs.statSync(path.join(pkg, 'dist/wasm/nlipsy.core.wasm')).size;
  process.stdout.write(`built dist/ — glue ${glue} bytes, core module ${core} bytes\n`);
}

main();
