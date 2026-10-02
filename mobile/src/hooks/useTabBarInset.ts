import { useContext } from 'react';
import { BottomTabBarHeightContext } from 'react-native-bottom-tabs';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

// Height of the floating native bottom tab bar, which overlays the scene.
// App.tsx pads each iOS tab root's SCENE by it (`useStackScreenOptions`), so
// the last rows — e.g. the collapsible "Done" group — end above the bar.
// Padding only the scroll content was not enough: the scroll view still ran
// under the bar, VoiceOver counted a row there as visible, and its double tap
// at the row's centre hit the bar instead.
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
