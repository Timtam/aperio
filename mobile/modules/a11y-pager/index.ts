import { requireNativeViewManager } from 'expo-modules-core';
import { type ComponentType, type ReactNode } from 'react';
import { type ViewProps } from 'react-native';

// Native container that intercepts VoiceOver's three-finger swipe
// (iOS `accessibilityScroll:`) and forwards the direction to JS, so the
// calendar views can page WITHOUT iOS's "page X of N" announcement (which a
// real UIScrollView emits) and without losing the reader's focus. See
// modules/a11y-pager/ios/A11yPagerModule.swift for the override. On Android the
// view is a plain pass-through (TalkBack has no equivalent scroll-to-page
// gesture and no page announcement), so callers use it on iOS only.

export interface A11yPagerViewProps extends ViewProps {
  /** Fired on a horizontal VoiceOver three-finger swipe. `next` = swipe left
   *  (advance a period), `prev` = swipe right. */
  onPage?: (event: {
    nativeEvent: { direction: 'prev' | 'next' };
  }) => void;
  children?: ReactNode;
}

export const A11yPagerView: ComponentType<A11yPagerViewProps> =
  requireNativeViewManager('A11yPager');
