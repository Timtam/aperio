#!/usr/bin/env node
/**
 * Every `host.<method>()` the Android bridge calls must exist in the COMMITTED
 * UniFFI bindings.
 *
 * Those bindings are generated from `crates/cal-ffi` and checked in, and the
 * vendoring step that refreshes the native `.so` did not refresh them — so they
 * drifted, silently, for weeks. Nothing catches that on this side of the fence:
 * `tsc` and ESLint never look at Kotlin, and the mismatch only surfaces as
 * `:cal-ffi:compileReleaseKotlin` failing five minutes into an EAS build,
 * long after the developer has moved on.
 *
 * Sixteen Host methods were missing when this was written — the whole event
 * group API and the whole day-marker API — which is to say two features that
 * looked finished could never have run on Android.
 *
 * Run: node mobile/scripts/check-android-bindings.mjs
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');

const BRIDGE = join(
  root,
  'mobile/modules/cal-ffi/android/src/main/java/expo/modules/calffi/CalFfiModule.kt',
);
const BINDINGS = join(
  root,
  'mobile/modules/cal-ffi/android/src/main/java/uniffi/cal_ffi/cal_ffi.kt',
);

const bridge = readFileSync(BRIDGE, 'utf8');
const bindings = readFileSync(BINDINGS, 'utf8');

// `host.foo(`, and the backtick form UniFFI emits for Kotlin keywords.
const called = new Set(
  [...bridge.matchAll(/\bhost\.`?([A-Za-z][A-Za-z0-9_]*)`?\s*\(/g)].map((m) => m[1]),
);

// Declared in the generated interface/class.
const declared = new Set(
  [...bindings.matchAll(/fun\s+`?([A-Za-z][A-Za-z0-9_]*)`?\s*\(/g)].map((m) => m[1]),
);

const missing = [...called].filter((name) => !declared.has(name)).sort();

if (missing.length > 0) {
  console.error(
    `The Android bridge calls ${missing.length} Host method(s) the committed ` +
      `UniFFI bindings do not declare:\n`,
  );
  for (const name of missing) console.error(`  host.${name}()`);
  console.error(
    '\nThe bindings are stale. Regenerate them (see mobile/README.md):\n' +
      '  cargo build -p cal-ffi\n' +
      '  cargo run -p cal-ffi --features cli --bin uniffi-bindgen -- \\n' +
      '    generate --library target/debug/cal_ffi.dll --language kotlin --out-dir <tmp>\n' +
      '  copy <tmp>/uniffi/cal_ffi/cal_ffi.kt over the committed one\n\n' +
      'The committed bindings and the vendored .so must come from the SAME\n' +
      'cal-ffi source, or JNA fails to resolve symbols at call time.',
  );
  process.exit(1);
}

console.log(
  `Android bindings OK — all ${called.size} Host methods the bridge calls are declared.`,
);
