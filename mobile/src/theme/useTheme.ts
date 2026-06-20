import { useContext, useMemo } from 'react';

import { ThemeContext } from './context';
import type { Theme, ThemeColors } from './tokens';

/** The active resolved theme (mode + colours). */
export function useTheme(): Theme {
  return useContext(ThemeContext);
}

/**
 * Build a themed StyleSheet from a factory. RN's `StyleSheet.create` runs at
 * module level, so static styles can't read a runtime theme; this rebuilds them
 * whenever the mode changes (memoised on the colour set, which is a stable
 * per-mode reference, so it only recomputes on an actual mode switch).
 *
 *   const styles = useThemedStyles((c) =>
 *     StyleSheet.create({ screen: { backgroundColor: c.background } }),
 *   );
 */
export function useThemedStyles<T>(factory: (colors: ThemeColors) => T): T {
  const { colors } = useTheme();
  return useMemo(() => factory(colors), [colors, factory]);
}
