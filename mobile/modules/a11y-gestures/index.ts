import { NativeModule, requireNativeModule } from 'expo';

// App-wide VoiceOver gesture host. The native module
// (modules/a11y-gestures/ios/A11yGesturesModule.swift) adds overrides to
// UIWindow at run time that catch a magic tap (two-finger double-tap) and a
// horizontal three-finger scroll AT THE WINDOW — which is where the gesture ends
// up when VoiceOver focus is on a UIAccessibilityElement (a heading / chrome
// control) that never bubbles the gesture to a mid-tree in-content view. It
// emits `magicTap` / `page` events; src/a11y/gestureHost.ts routes them to the
// focused screen. It also carries the form lab of a diagnostic build
// (`setLabFlags`, `labLog`; src/a11y/formLab.ts). iOS only fires these; the
// Android module is a never-firing stub for parity, and the lab is never called
// there.

export type A11yGesturesEvents = {
  magicTap: () => void;
  page: (event: { direction: 'prev' | 'next' }) => void;
  labLog: (event: { line: string }) => void;
};

declare class A11yGesturesNativeModule extends NativeModule<A11yGesturesEvents> {
  /** iOS only: the form lab's switches (src/a11y/formLab.ts). */
  setLabFlags(flags: Record<string, string | boolean>): void;
}

export const A11yGestures =
  requireNativeModule<A11yGesturesNativeModule>('A11yGestures');
