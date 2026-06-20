import {
  DarkTheme,
  DefaultTheme,
  type Theme as NavigationTheme,
} from '@react-navigation/native';

import type { Theme } from './tokens';

/**
 * Derive a React Navigation theme (header / tab-bar / card chrome) from our
 * tokens so the nav chrome tracks the same mode as the screen content. Built on
 * top of Default/DarkTheme so the v7-required `fonts` block is carried through;
 * only the colours are overridden.
 */
export function navigationThemeFor(theme: Theme): NavigationTheme {
  const base = theme.mode === 'light' ? DefaultTheme : DarkTheme;
  return {
    ...base,
    dark: theme.mode !== 'light',
    colors: {
      ...base.colors,
      primary: theme.colors.accent,
      background: theme.colors.background,
      card: theme.colors.surfaceAlt,
      text: theme.colors.textPrimary,
      border: theme.colors.border,
      notification: theme.colors.danger,
    },
  };
}
