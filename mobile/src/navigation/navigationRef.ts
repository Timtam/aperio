import { CommonActions, createNavigationContainerRef } from '@react-navigation/native';

import type { RootStackParamList, RootTabParamList } from './types';

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

/**
 * Navigate from outside a navigator to a screen NESTED inside a specific tab's
 * stack — `navigate(tab, { screen, params })`. A bare `navigateRoot('TaskEditor',
 * …)` resolves through whichever focused stack registers `TaskEditor`, but
 * fails (unhandled) when the focused tab's stack does NOT register it (e.g. the
 * Contacts stack has no TaskEditor). Targeting the owning tab explicitly opens
 * the screen regardless of which tab is focused.
 *
 * Typed against the tab list + the nested screen's params; dispatched via
 * `CommonActions.navigate` (the container ref's generic is the root stack, which
 * doesn't carry the tab keys, so the action is the type-clean way to express the
 * nested payload). A no-op until the container is ready, like `navigateRoot`.
 */
export function navigateNested<ScreenName extends keyof RootStackParamList>(
  tab: keyof RootTabParamList,
  screen: ScreenName,
  params: RootStackParamList[ScreenName],
): void {
  if (navigationRef.isReady()) {
    navigationRef.dispatch(CommonActions.navigate(tab, { screen, params }));
  }
}
