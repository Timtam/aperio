import { fileURLToPath, URL } from 'node:url';

import { transformWithEsbuild, type Plugin } from 'vite';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

/** Mobile sources a desktop test runs — the task-settings contract runs the
 *  mobile module on every row. */
const MOBILE_SOURCE = /[\\/]mobile[\\/]src[\\/].*\.tsx?$/;

/**
 * Compile the mobile sources without looking up their `tsconfig.json`.
 *
 * Vite hands every TypeScript file to esbuild with the nearest tsconfig, and
 * `mobile/tsconfig.json` extends `expo/tsconfig.base` — which only resolves
 * where the mobile packages are installed. CI's Frontend job does not install
 * them, so loading any mobile file failed there ("failed to resolve extends")
 * while it passed on a machine that had them. The mobile modules a test loads
 * are plain TypeScript and need nothing from that tsconfig; an inline one
 * (a string, which Vite takes instead of reading files) keeps the lookup away.
 */
function mobileSourcesWithoutExpoTsconfig(): Plugin {
  return {
    name: 'aperio:mobile-sources-without-expo-tsconfig',
    enforce: 'pre',
    async transform(code, id) {
      const file = id.split('?')[0];
      if (!MOBILE_SOURCE.test(file)) return null;
      const result = await transformWithEsbuild(code, file, {
        loader: file.endsWith('x') ? 'tsx' : 'ts',
        format: 'esm',
        target: 'esnext',
        sourcemap: true,
        tsconfigRaw: '{"compilerOptions":{}}',
      });
      return { code: result.code, map: result.map };
    },
  };
}

export default defineConfig({
  plugins: [mobileSourcesWithoutExpoTsconfig(), react()],
  // The plugin above compiles the mobile sources; Vite's own step leaves them.
  esbuild: { exclude: [MOBILE_SOURCE] },
  resolve: {
    alias: {
      // Mirror vite.config.ts so the shared translations resolve under test.
      '@aperio/locales': fileURLToPath(new URL('./locales', import.meta.url)),
      // Mirror vite.config.ts so the shared frontend domain resolves under test.
      '@aperio/shared': fileURLToPath(
        new URL('./shared/index.ts', import.meta.url),
      ),
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    setupFiles: ['src/test-setup.ts'],
    // The dialog tests render whole editors, and the suite runs them in
    // parallel workers: a test that takes ~2 s alone can sit well past the 5 s
    // default while the machine is busy elsewhere. The timeout is only how
    // long a test is ALLOWED to take, so raising it costs nothing on a green
    // run and stops a loaded machine from reporting failures that are not.
    testTimeout: 20_000,
  },
});
