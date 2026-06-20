import { useContext } from 'react';
import { BottomTabBarHeightContext } from 'react-native-bottom-tabs';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

// Bottom padding a tab-root screen's scroll content needs so its last rows
// clear the floating native bottom tab bar (the bar overlays the scene, so
// without this the final row — e.g. the collapsible "Done" group — sits under
// the bar and can't be tapped).
//
// The native bar reports its measured height into BottomTabBarHeightContext via
// onTabBarMeasured. We read the context directly rather than through
// react-native-bottom-tabs' `useBottomTabBarHeight`, which THROWS when the
// context is absent — reading it ourselves lets a screen rendered outside the
// tabs fall back gracefully instead of crashing. Before the first native
// measurement (height still 0) we fall back to the bottom safe-area inset plus a
// typical bar height so content is never briefly hidden.
export function useTabBarInset(): number {
  const measured = useContext(BottomTabBarHeightContext) ?? 0;
  const insets = useSafeAreaInsets();
  return measured > 0 ? measured : insets.bottom + 56;
}
