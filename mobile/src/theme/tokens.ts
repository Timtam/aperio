// Theme tokens — the mobile twin of the desktop CSS custom properties in
// src/styles.css. Light vs dark follows the device-local theme-mode setting
// (see themeMode.ts: 'system' — the default — tracks the OS via
// useColorScheme; 'light'/'dark' pin the palette, mirroring the desktop's
// data-theme resolution in src/state/themeMode.ts). The platform
// high-contrast accessibility flag switches to the high-contrast palette and
// wins over any choice, matching the desktop's `forced-colors: active`
// override. See [[ThemeProvider]].
//
// The LIGHT palette is byte-identical to the hexes the screens hardcoded before
// this migration, so light mode has ZERO visual regression; dark + high-contrast
// are purely additive. A handful of near-duplicate one-off shades the screens
// used (e.g. two slightly different delete-button backgrounds) are deliberately
// unified onto the canonical token here.

/** Active appearance mode, as RESOLVED by ThemeProvider: the device-local
 *  theme-mode choice (or the OS scheme when it's 'system'), with the
 *  platform high-contrast flag overriding both. */
export type ThemeMode = 'light' | 'dark' | 'highContrast';

/**
 * Semantic colour tokens. Components reference these by ROLE, never by raw hex,
 * so a single token swap re-themes the whole app. Grouped: surfaces, borders,
 * text, the primary-action (accent) family, and the status families.
 */
export interface ThemeColors {
  /** Screen background. */
  background: string;
  /** Input / form-field background. */
  surface: string;
  /** Card / row / ghost-button / view-switcher background. */
  surfaceAlt: string;
  /** Extra-light tint for nested fills. */
  surfaceSubtle: string;
  /** Selected-option background (accent-tinted). */
  surfaceSelected: string;
  /** Pressed-state feedback background. */
  surfacePressed: string;
  /** Default border / divider / switch track (off). */
  border: string;
  /** Subtle overlay border drawn over arbitrary fills (e.g. colour dots). */
  borderOverlay: string;
  /** Primary text (strongest contrast). */
  textPrimary: string;
  /** Secondary / muted / hint / disabled / meta text. */
  textSecondary: string;
  /** Form-field labels and group legends. */
  textLabel: string;
  /** Text drawn on top of the accent colour (primary buttons). */
  textOnAccent: string;
  /** Ghost-button / link / "add" affordance text. */
  link: string;
  /** Primary action: button background, selected border, switch track (on). */
  accent: string;
  /** Primary action, pressed. */
  accentPressed: string;
  /** Primary action, disabled. */
  accentDisabled: string;
  /** Danger / destructive / error text + icons. */
  danger: string;
  /** Danger background (light tint). */
  dangerBg: string;
  /** Danger border. */
  dangerBorder: string;
  /** Warning text. */
  warning: string;
  /** Warning background. */
  warningBg: string;
}

/** A resolved theme: the active mode + its colour set. */
export interface Theme {
  mode: ThemeMode;
  colors: ThemeColors;
}

// ── Light (byte-identical to the pre-migration hardcoded palette) ────────────
const lightColors: ThemeColors = {
  background: '#ffffff',
  surface: '#f8fafc',
  surfaceAlt: '#f4f7fb',
  surfaceSubtle: '#eef2f8',
  surfaceSelected: '#eaf0fd',
  surfacePressed: '#e4ebf5',
  border: '#c9d2e0',
  borderOverlay: 'rgba(0, 0, 0, 0.18)',
  textPrimary: '#10131a',
  textSecondary: '#5b6573',
  textLabel: '#2b3240',
  textOnAccent: '#ffffff',
  link: '#1d3a2f',
  accent: '#1d4ed8',
  accentPressed: '#1740a8',
  accentDisabled: '#9aa9c9',
  danger: '#b42318',
  dangerBg: '#fbeceb',
  dangerBorder: '#d9b3b0',
  warning: '#92400e',
  warningBg: '#fef3c7',
};

// ── Dark (designed for WCAG-AA contrast on the dark surfaces) ────────────────
const darkColors: ThemeColors = {
  background: '#11151c',
  surface: '#1a1f29',
  surfaceAlt: '#1e2531',
  surfaceSubtle: '#161b23',
  surfaceSelected: '#1c2e52',
  surfacePressed: '#283142',
  border: '#3a4456',
  borderOverlay: 'rgba(255, 255, 255, 0.24)',
  textPrimary: '#eef1f6',
  textSecondary: '#a3adbd',
  textLabel: '#c8d0dd',
  // The accent is a light blue in dark mode, so on-accent text is dark.
  textOnAccent: '#0a1220',
  link: '#7fdcc0',
  accent: '#5b9bff',
  accentPressed: '#7db0ff',
  accentDisabled: '#41506b',
  danger: '#ff8a80',
  dangerBg: '#3a1d1c',
  dangerBorder: '#7a3a36',
  warning: '#ffcf7a',
  warningBg: '#3a2e15',
};

// ── High-contrast (near-pure black/white, bright accents, strong borders) ────
// The mobile analogue of the desktop's `forced-colors: active` palette. One
// dark-based set (the common Windows high-contrast shape), not a light/dark
// pair — the OS exposes a single "increase contrast" flag per platform.
const highContrastColors: ThemeColors = {
  background: '#000000',
  surface: '#000000',
  surfaceAlt: '#0d0d0d',
  surfaceSubtle: '#000000',
  surfaceSelected: '#003e7e',
  surfacePressed: '#1a1a1a',
  border: '#ffffff',
  borderOverlay: 'rgba(255, 255, 255, 0.7)',
  textPrimary: '#ffffff',
  textSecondary: '#e6e6e6',
  textLabel: '#ffffff',
  textOnAccent: '#000000',
  link: '#6cd0ff',
  accent: '#4da3ff',
  accentPressed: '#7dbcff',
  accentDisabled: '#888888',
  danger: '#ff6b6b',
  dangerBg: '#2a0000',
  dangerBorder: '#ff6b6b',
  warning: '#ffd24d',
  warningBg: '#2a2200',
};

/** Stable `Theme` object per mode — referenced as the `useThemedStyles` memo
 *  key, so the StyleSheet only rebuilds when the mode actually changes. */
export const THEMES: Record<ThemeMode, Theme> = {
  light: { mode: 'light', colors: lightColors },
  dark: { mode: 'dark', colors: darkColors },
  highContrast: { mode: 'highContrast', colors: highContrastColors },
};
