import { requireNativeViewManager } from 'expo-modules-core';
import { type ComponentType, type ReactNode } from 'react';
import { type ViewProps } from 'react-native';

// Native container that catches VoiceOver's MAGIC TAP (a two-finger double-tap)
// and forwards it to JS, bypassing React Native's `onMagicTap` prop — which is
// unwired on the New Architecture (Fabric reads a raw `onAccessibilityMagicTap`
// prop, but JS still sends the legacy name `onMagicTap`, with no alias, so it
// never fires). The iOS view overrides `accessibilityPerformMagicTap`; on Android
// it is a plain pass-through (TalkBack has no magic-tap gesture). See
// modules/a11y-magic-tap/ios/A11yMagicTapModule.swift. Consumers use the
// `MagicTapView` wrapper (src/components/MagicTapView.tsx), which renders this on
// iOS only.

export interface A11yMagicTapViewProps extends Omit<ViewProps, 'onMagicTap'> {
  /** Fired on a VoiceOver two-finger double-tap anywhere within the content.
   *  (React Native's own `onMagicTap` is a no-arg `() => void`; the native view
   *  delivers it as an Expo view event with a `nativeEvent` payload instead.) */
  onMagicTap?: (event: { nativeEvent: Record<string, never> }) => void;
  children?: ReactNode;
}

export const A11yMagicTapView: ComponentType<A11yMagicTapViewProps> =
  requireNativeViewManager('A11yMagicTap');
