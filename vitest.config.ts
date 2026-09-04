import { fileURLToPath, URL } from 'node:url';

import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
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
