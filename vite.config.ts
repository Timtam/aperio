import { fileURLToPath, URL } from 'node:url';

import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri-specific configuration — see https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],

  resolve: {
    alias: {
      // Shared UI translations (also consumed by the mobile app). Kept as a
      // top-level package so both frontends draw from one source of truth.
      '@aperio/locales': fileURLToPath(new URL('./locales', import.meta.url)),
    },
  },

  // Tauri expects a fixed port and bails out if the dev server moves.
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 5174,
        }
      : undefined,
    watch: {
      // Exclude Tauri-side sources from the Vite watcher.
      ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**'],
    },
  },

  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target:
      process.env.TAURI_ENV_PLATFORM === 'windows'
        ? 'chrome105'
        : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
