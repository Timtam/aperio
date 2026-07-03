import { useState } from 'react';

/**
 * Device-local light/dark/system theme mode.
 *
 * `system` (the default) follows the OS via `prefers-color-scheme` and tracks
 * live changes; `light`/`dark` pin the palette. The RESOLVED theme is written
 * to `<html data-theme="light|dark">`, which is what styles.css keys its dark
 * token block (and `color-scheme`) on — so the CSS never needs the media query
 * and the JS stays the single resolver.
 *
 * Stored per-device in `localStorage` (a bright office desktop and an OLED
 * laptop want different modes), NOT synced — mirroring the UI-scale setting.
 * Read synchronously and applied BEFORE the first paint (see main.tsx) so the
 * UI never flashes in the wrong palette.
 */

export type ThemeMode = 'system' | 'light' | 'dark';

const STORAGE_KEY = 'aperio.ui.themeMode';
export const DEFAULT_THEME_MODE: ThemeMode = 'system';

/** The system-follow listener while mode === 'system', so an OS switch
 *  retints the app live. Replaced/removed on every apply. */
let systemListener: {
  query: MediaQueryList;
  handler: (e: MediaQueryListEvent) => void;
} | null = null;

function isThemeMode(raw: string | null): raw is ThemeMode {
  return raw === 'system' || raw === 'light' || raw === 'dark';
}

/** The persisted mode, or the default. Synchronous — safe before first paint. */
export function readThemeMode(): ThemeMode {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return isThemeMode(raw) ? raw : DEFAULT_THEME_MODE;
  } catch {
    return DEFAULT_THEME_MODE;
  }
}

function setDocumentTheme(dark: boolean): void {
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
}

/** Resolve + apply the mode to the document, and keep following the OS
 *  while the mode is 'system'. */
export function applyThemeMode(mode: ThemeMode): void {
  if (systemListener) {
    systemListener.query.removeEventListener('change', systemListener.handler);
    systemListener = null;
  }
  const query = window.matchMedia('(prefers-color-scheme: dark)');
  if (mode === 'system') {
    setDocumentTheme(query.matches);
    const handler = (e: MediaQueryListEvent) => setDocumentTheme(e.matches);
    query.addEventListener('change', handler);
    systemListener = { query, handler };
    return;
  }
  setDocumentTheme(mode === 'dark');
}

/** Persist the mode and apply it live. */
export function writeThemeMode(mode: ThemeMode): void {
  try {
    localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // localStorage unavailable (private mode) — still apply for this session.
  }
  applyThemeMode(mode);
}

/** Settings-control state: the current mode + a persisting+applying setter. */
export function useThemeMode(): [ThemeMode, (mode: ThemeMode) => void] {
  const [mode, setMode] = useState<ThemeMode>(() => readThemeMode());
  const update = (next: ThemeMode) => {
    setMode(next);
    writeThemeMode(next);
  };
  return [mode, update];
}
