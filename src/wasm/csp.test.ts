import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

/**
 * The shipped app may compile the core rules' WebAssembly.
 *
 * `main.tsx` instantiates the module before anything renders, and a webview
 * compiles WebAssembly only when the page's Content Security Policy allows it:
 * `'wasm-unsafe-eval'` in `script-src`. Without it every desktop build since
 * the WebAssembly door opened failed at startup with "Aperio konnte seine
 * Kernregeln nicht laden". Nothing else caught it: `tauri dev` serves the page
 * without that policy, and these tests instantiate the module in Node.
 */
describe('the desktop Content Security Policy', () => {
  const conf = JSON.parse(
    readFileSync(resolve(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8'),
  ) as { app: { security: { csp: string } } };
  const directives = new Map(
    conf.app.security.csp
      .split(';')
      .map((d) => d.trim().toLowerCase().split(/\s+/))
      .filter((parts) => parts[0])
      .map(([name, ...sources]) => [name, sources] as const),
  );
  // No fallback to default-src, although a browser would take one: Tauri
  // always writes its own script-src into the page's policy ('self' plus the
  // hashes of the scripts it ships, `replace_csp_nonce` in tauri's manager),
  // so a source that sits only in default-src never reaches the webview.
  const scriptSrc = directives.get('script-src') ?? [];

  it('lets the webview compile WebAssembly', () => {
    expect(scriptSrc).toContain("'wasm-unsafe-eval'");
  });

  it('allows WebAssembly and nothing broader', () => {
    // `'unsafe-eval'` would also compile WebAssembly, and every string handed
    // to eval() or new Function() with it.
    expect(scriptSrc).not.toContain("'unsafe-eval'");
    expect(scriptSrc).not.toContain("'unsafe-inline'");
  });
});
