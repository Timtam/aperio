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
  },
});
