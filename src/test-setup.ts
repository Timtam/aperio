// Test environment bootstrap.
// jsdom does not implement `matchMedia`. Stub it so hooks that probe
// prefers-color-scheme / prefers-reduced-motion can run.
import '@testing-library/jest-dom/vitest';

// Pin the test language so component assertions stay deterministic — the
// app default is now the *system* language (jsdom would resolve to English),
// but the existing tests assert the German UI strings.
import i18n from './i18n';
void i18n.changeLanguage('de');

if (typeof window !== 'undefined' && !window.matchMedia) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}
