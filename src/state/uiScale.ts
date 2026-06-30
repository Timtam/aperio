import { useState } from 'react';

/**
 * Device-local UI scale (fonts + spacing together).
 *
 * The desktop type AND spacing systems are rem-based (`--font-size-*`,
 * `--space-*` in styles.css) against the default 16px root, and the calendar
 * hour-grid is rem too, so scaling the document root font-size scales the whole
 * interface proportionally — like browser zoom — with no per-component work.
 *
 * Stored per-device in `localStorage` (a 4K desktop and a laptop want different
 * scales), NOT synced. Read synchronously and applied to `documentElement`
 * BEFORE the first paint (see main.tsx) so the UI never flashes at 100% and
 * then jumps.
 */

const STORAGE_KEY = 'aperio.ui.fontScale';
const BASE_FONT_PX = 16;

export const DEFAULT_UI_SCALE = 1;

/** Font-size slider bounds + granularity: 70%–200% in 5% steps. */
export const UI_MIN_SCALE = 0.7;
export const UI_MAX_SCALE = 2;
export const UI_SCALE_STEP = 0.05;

function clampScale(n: number): number {
  if (!Number.isFinite(n)) return DEFAULT_UI_SCALE;
  return Math.min(UI_MAX_SCALE, Math.max(UI_MIN_SCALE, n));
}

/** The persisted scale, or the default. Synchronous — safe before first paint. */
export function readUiScale(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_UI_SCALE;
    return clampScale(Number.parseFloat(raw));
  } catch {
    return DEFAULT_UI_SCALE;
  }
}

/** Drive the document root font-size so every rem token scales with it. */
export function applyUiScale(scale: number): void {
  document.documentElement.style.fontSize = `${BASE_FONT_PX * clampScale(scale)}px`;
}

/** Persist the scale and apply it live. */
export function writeUiScale(scale: number): void {
  const next = clampScale(scale);
  try {
    localStorage.setItem(STORAGE_KEY, String(next));
  } catch {
    // localStorage unavailable (private mode) — still apply for this session.
  }
  applyUiScale(next);
}

/** Settings-control state: the current scale + a persisting+applying setter. */
export function useUiScale(): [number, (scale: number) => void] {
  const [scale, setScale] = useState<number>(() => readUiScale());
  const update = (next: number) => {
    setScale(next);
    writeUiScale(next);
  };
  return [scale, update];
}
