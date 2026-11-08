#!/usr/bin/env node
/**
 * Generate the UniFFI Kotlin bindings from `crates/cal-ffi`, into the place the
 * Android module compiles them from:
 * `mobile/modules/cal-ffi/android/src/main/java/uniffi/` (gitignored, TODO 89a).
 *
 * They used to be checked in, and nothing refreshed them but a manual step:
 * the vendoring step that rebuilds the native `.so` did not, so a stale file
 * surfaced as a Kotlin compile failure minutes into an EAS build, or as a
 * checksum panic on the device. Like the Swift bindings, which never were in
 * the repository, they are now generated wherever they are needed: here for a
 * local Android build and for `check-ffi-bridges.mjs`, and by the Android
 * workflow next to the release `.so`.
 *
 * The bindings carry a checksum per method that the native library checks at
 * load time, so they must come from the same `cal-ffi` source as the `.so`
 * that ships with them. This builds the host library (`cargo build -p cal-ffi
 * --features cli`) and reads the interface out of it; the `.so` cargo-ndk
 * builds from the same source has the same checksums.
 *
 * Run: node mobile/scripts/kotlin-bindings.mjs
 */

import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const ROOT = join(here, '..', '..');

/** Where the Android module compiles the bindings from. */
export const BINDINGS_DIR = join(ROOT, 'mobile/modules/cal-ffi/android/src/main/java/uniffi');
export const BINDINGS_FILE = join(BINDINGS_DIR, 'cal_ffi/cal_ffi.kt');

/** The host library cargo builds, by platform. */
function hostLibrary() {
  const target = process.env.CARGO_TARGET_DIR ?? join(ROOT, 'target');
  const name =
    process.platform === 'win32'
      ? 'cal_ffi.dll'
      : process.platform === 'darwin'
        ? 'libcal_ffi.dylib'
        : 'libcal_ffi.so';
  return join(target, 'debug', name);
}

function cargo(args) {
  execFileSync('cargo', args, { cwd: ROOT, stdio: 'inherit' });
}

/** Build cal-ffi, generate the Kotlin bindings, and put them in place. */
export function generateKotlinBindings() {
  cargo(['build', '-p', 'cal-ffi', '--features', 'cli']);
  const library = hostLibrary();
  if (!existsSync(library)) {
    throw new Error(`cargo built no ${library}; cannot generate the Kotlin bindings`);
  }
  const out = mkdtempSync(join(tmpdir(), 'aperio-uniffi-kotlin-'));
  try {
    cargo([
      'run',
      '-q',
      '-p',
      'cal-ffi',
      '--features',
      'cli',
      '--bin',
      'uniffi-bindgen',
      '--',
      'generate',
      '--library',
      library,
      '--language',
      'kotlin',
      '--no-format',
      '--out-dir',
      out,
    ]);
    const generated = join(out, 'uniffi');
    if (!existsSync(join(generated, 'cal_ffi', 'cal_ffi.kt'))) {
      throw new Error(`uniffi-bindgen wrote no cal_ffi.kt into ${out}`);
    }
    rmSync(BINDINGS_DIR, { recursive: true, force: true });
    cpSync(generated, BINDINGS_DIR, { recursive: true });
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
  return BINDINGS_FILE;
}

// Run as a script, not imported (check-ffi-bridges.mjs imports it).
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  console.log(`Kotlin bindings written to ${generateKotlinBindings()}`);
}
