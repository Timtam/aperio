import { NativeModule, requireNativeModule } from 'expo-modules-core';

// App-wide VoiceOver gesture host. The native UIWindow category
// (modules/a11y-gestures/ios/AperioWindowGestures.m) catches a magic tap
// (two-finger double-tap) and a horizontal three-finger scroll AT THE WINDOW —
// which is where the gesture ends up when VoiceOver focus is on a
// UIAccessibilityElement (a heading / chrome control) that never bubbles the
// gesture to a mid-tree in-content view. It emits `magicTap` / `page` events;
// src/a11y/gestureHost.ts routes them to the focused screen. iOS only fires
// these; the Android module is a never-firing stub for parity.

export type A11yGesturesEvents = {
  magicTap: () => void;
  page: (event: { direction: 'prev' | 'next' }) => void;
};

declare class A11yGesturesNativeModule extends NativeModule<A11yGesturesEvents> {}

export const A11yGestures =
  requireNativeModule<A11yGesturesNativeModule>('A11yGestures');
