import { createNavigationContainerRef } from '@react-navigation/native';

import type { RootStackParamList } from './types';

/**
 * App-level navigation ref — lets components mounted INSIDE the
 * `NavigationContainer` but OUTSIDE any navigator (e.g. the day-start review
 * modal, which overlays whatever tab is focused) drive navigation. Such
 * components have no navigator context, so `useNavigation()` would throw; the
 * container ref is the supported escape hatch.
 *
 * Attached to the `NavigationContainer` in App.tsx.
 */
export const navigationRef = createNavigationContainerRef<RootStackParamList>();

/**
 * Navigate from outside a navigator via the container ref. A no-op until the
 * container is mounted + ready (so an early call can't throw); by the time any
 * UI the user can act on is on screen, the container is long since ready.
 */
export function navigateRoot<RouteName extends keyof RootStackParamList>(
  ...args: RouteName extends unknown
    ? undefined extends RootStackParamList[RouteName]
      ? [screen: RouteName] | [screen: RouteName, params: RootStackParamList[RouteName]]
      : [screen: RouteName, params: RootStackParamList[RouteName]]
    : never
): void {
  if (navigationRef.isReady()) {
    // The overload signature above mirrors React Navigation's own
    // `navigate(name, params)` shape, so this forward is type-safe.
    navigationRef.navigate(...args);
  }
}
