#!/usr/bin/env node
/**
 * The Rust host, the committed UniFFI bindings, and the two native bridges all
 * have to agree about every method — that it EXISTS and how many arguments it
 * takes.
 *
 * The bindings are generated from `crates/cal-ffi` and checked in, and the
 * vendoring step that refreshes the native `.so` does not refresh them. Nothing
 * else on this side of the fence looks: `tsc` and ESLint never read Kotlin or
 * Swift. Sixteen Host methods were missing when the first version of this
 * script was written — the whole event-group API and the whole day-marker API,
 * two features that looked finished and could never have run on Android.
 *
 * That version compared NAMES only, and a later change slipped straight past
 * it: three Rust methods gained a `lang` parameter, the Kotlin bridge was
 * updated to pass it, and the regeneration step silently did not run. Every
 * name still matched, so the check said OK. What would have failed is
 * `:cal-ffi:compileReleaseKotlin`, minutes into an EAS build — and on iOS the
 * same class of mistake is only visible after an XCFramework build, which is
 * the most expensive feedback loop in this project.
 *
 * So it now checks arity, from three directions:
 *
 *   Rust  ↔ bindings  — the bindings are stale (or were generated elsewhere).
 *   bindings ↔ Kotlin — the Android bridge calls a shape that does not exist.
 *   Rust  ↔ Swift     — ditto for iOS, where nothing local ever compiles it.
 *
 * Only methods a bridge actually calls are checked, so a Rust method no phone
 * uses is nobody's problem here.
 *
 * Run: node mobile/scripts/check-ffi-bridges.mjs
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');

const RUST = join(root, 'crates/cal-ffi/src/host.rs');
const BINDINGS = join(
  root,
  'mobile/modules/cal-ffi/android/src/main/java/uniffi/cal_ffi/cal_ffi.kt',
);
const KOTLIN = join(
  root,
  'mobile/modules/cal-ffi/android/src/main/java/expo/modules/calffi/CalFfiModule.kt',
);
const SWIFT = join(root, 'mobile/modules/cal-ffi/ios/CalFfiModule.swift');

/** snake_case as Rust writes it → camelCase as UniFFI emits it. */
const camel = (name) => name.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());

/**
 * The text between the parenthesis at `open` and its match, plus the index just
 * past the closing one. Depth-aware, so a generic or a nested call inside an
 * argument does not end it early.
 */
function balanced(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    const c = text[i];
    if (c === '(') depth += 1;
    else if (c === ')') {
      depth -= 1;
      if (depth === 0) return { inner: text.slice(open + 1, i), end: i + 1 };
    }
  }
  return null;
}

/**
 * Top-level comma-separated items. `<>`, `()` and `[]` all nest, so
 * `Map<String, Int>` and `foo(a, b)` each count as one.
 */
function splitTop(text) {
  const parts = [];
  let depth = 0;
  let current = '';
  for (const c of text) {
    if (c === '<' || c === '(' || c === '[') depth += 1;
    if (c === '>' || c === ')' || c === ']') depth -= 1;
    if (c === ',' && depth === 0) {
      parts.push(current);
      current = '';
    } else current += c;
  }
  parts.push(current);
  return parts.map((p) => p.trim()).filter((p) => p.length > 0);
}

/** The `#[uniffi::export] impl Host { … }` block — the methods a phone can call. */
function exportedImpl(rust) {
  const marker = rust.indexOf('#[uniffi::export]\nimpl Host {');
  if (marker < 0) return null;
  const open = rust.indexOf('{', marker);
  let depth = 0;
  for (let i = open; i < rust.length; i += 1) {
    if (rust[i] === '{') depth += 1;
    else if (rust[i] === '}') {
      depth -= 1;
      if (depth === 0) return rust.slice(open, i);
    }
  }
  return null;
}

/** camelCase name → argument count, from the Rust source. `self` does not count. */
function rustArity(rust) {
  const block = exportedImpl(rust);
  if (block === null) {
    console.error(
      'Could not find the `#[uniffi::export] impl Host` block in\n  ' +
        RUST +
        '\nThe check cannot run, which is a failure and not a pass.',
    );
    process.exit(1);
  }
  const out = new Map();
  for (const m of block.matchAll(/\bpub (?:async )?fn\s+([a-z][a-z0-9_]*)\s*\(/g)) {
    const args = balanced(block, m.index + m[0].length - 1);
    if (!args) continue;
    const params = splitTop(args.inner).filter(
      (p) => !/^(?:&(?:mut )?)?self$/.test(p.replace(/^&\s*/, '&')),
    );
    out.set(camel(m[1]), params.length);
  }
  return out;
}

/**
 * camelCase name → declared argument count, from `HostInterface` in the
 * committed bindings.
 *
 * Scoped to that one interface on purpose. The file also declares the callback
 * interfaces the host calls back INTO — `DeviceCalendarProviderInterface` among
 * them — and they share method names with Host: both have a `deleteEvent`, one
 * with a single parameter and one with three. A file-wide scan picks whichever
 * comes first and reports a disagreement that is not one.
 */
function bindingArity(bindings) {
  const start = bindings.indexOf('public interface HostInterface {');
  if (start < 0) {
    console.error(
      'Could not find `public interface HostInterface` in\n  ' +
        BINDINGS +
        '\nThe check cannot run, which is a failure and not a pass.',
    );
    process.exit(1);
  }
  const open = bindings.indexOf('{', start);
  let depth = 0;
  let end = bindings.length;
  for (let i = open; i < bindings.length; i += 1) {
    if (bindings[i] === '{') depth += 1;
    else if (bindings[i] === '}') {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  const block = bindings.slice(open, end);
  const out = new Map();
  for (const m of block.matchAll(/\bfun\s+`?([A-Za-z][A-Za-z0-9_]*)`?\s*\(/g)) {
    const args = balanced(block, m.index + m[0].length - 1);
    if (!args) continue;
    out.set(m[1], splitTop(args.inner).length);
  }
  return out;
}

/** camelCase name → argument count passed, from a bridge's `host.<name>(…)` calls. */
function callArity(source, pattern) {
  const out = new Map();
  for (const m of source.matchAll(pattern)) {
    const args = balanced(source, m.index + m[0].length - 1);
    if (!args) continue;
    const count = splitTop(args.inner).length;
    // A method called twice with different shapes is itself the bug.
    const seen = out.get(m[1]);
    out.set(m[1], seen === undefined ? count : Math.max(seen, count));
  }
  return out;
}

const rust = rustArity(readFileSync(RUST, 'utf8'));
const declared = bindingArity(readFileSync(BINDINGS, 'utf8'));
const kotlin = callArity(
  readFileSync(KOTLIN, 'utf8'),
  /\bhost\.`?([A-Za-z][A-Za-z0-9_]*)`?\s*\(/g,
);
const swift = callArity(
  readFileSync(SWIFT, 'utf8'),
  /\bself\.host\.([A-Za-z][A-Za-z0-9_]*)\s*\(/g,
);

const problems = [];

for (const [name, count] of kotlin) {
  if (!declared.has(name)) {
    problems.push(
      `the Android bridge calls host.${name}(), which the committed bindings do not declare`,
    );
    continue;
  }
  if (declared.get(name) !== count) {
    problems.push(
      `the Android bridge passes ${count} argument(s) to host.${name}(), ` +
        `but the committed bindings declare ${declared.get(name)}`,
    );
  }
}

for (const [name, count] of new Map([...kotlin, ...swift])) {
  if (!rust.has(name)) {
    problems.push(
      `a bridge calls host.${name}(), which is not in the exported Rust impl — ` +
        'either it was renamed or it is not exported over UniFFI',
    );
    continue;
  }
  if (rust.get(name) !== count) {
    const which = kotlin.get(name) === count ? 'Android' : 'iOS';
    problems.push(
      `the ${which} bridge passes ${count} argument(s) to host.${name}(), ` +
        `but crates/cal-ffi declares ${rust.get(name)}`,
    );
  }
}

for (const [name, count] of declared) {
  if (!rust.has(name)) continue;
  if (rust.get(name) !== count) {
    problems.push(
      `the committed bindings declare host.${name}() with ${count} argument(s), ` +
        `but crates/cal-ffi declares ${rust.get(name)} — the bindings are stale`,
    );
  }
}

// A parse that matched nothing would report no problems and mean nothing.
const floors = [
  ['exported Rust methods', rust.size, 100],
  ['declared bindings', declared.size, 100],
  ['Android bridge calls', kotlin.size, 100],
  ['iOS bridge calls', swift.size, 20],
];
for (const [what, found, floor] of floors) {
  if (found < floor) {
    console.error(
      `Only found ${found} ${what} — the parse is wrong, so this check proves nothing.`,
    );
    process.exit(1);
  }
}

if (problems.length > 0) {
  console.error(
    `The Rust host, the committed UniFFI bindings and the native bridges ` +
      `disagree in ${problems.length} place(s):\n`,
  );
  for (const p of [...new Set(problems)].sort()) console.error(`  ${p}`);
  console.error(
    '\nIf the bindings are stale, regenerate them (see mobile/README.md):\n' +
      '  cargo build -p cal-ffi\n' +
      '  cargo run -p cal-ffi --features cli --bin uniffi-bindgen -- \\\n' +
      '    generate --library target/debug/cal_ffi.dll --language kotlin --out-dir <tmp>\n' +
      '  copy <tmp>/uniffi/cal_ffi/cal_ffi.kt over the committed one\n\n' +
      'The committed bindings and the vendored .so must come from the SAME\n' +
      'cal-ffi source, or JNA fails to resolve symbols at call time.',
  );
  process.exit(1);
}

console.log(
  `FFI bridges OK — ${kotlin.size} Android and ${swift.size} iOS calls agree ` +
    `with the committed bindings and with crates/cal-ffi.`,
);
