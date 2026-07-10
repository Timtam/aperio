import { type ReactNode, useRef, useState } from 'react';
import {
  type LayoutChangeEvent,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  Platform,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';

import { A11yPagerView } from '../../modules/a11y-pager';
import { useScreenReaderEnabled } from '../a11y/useScreenReaderEnabled';

// Horizontal swipe-to-page wrapper for the calendar views — lets users flick
// horizontally between periods.
//
// SIGHTED path: a horizontal ScrollView with THREE full-size pages — empty
// spacers either side of the live content (the middle). A horizontal swipe lands
// on a spacer → we shift the period (onPrev / onNext) and snap straight back to
// the middle, so paging feels "infinite". Only the middle renders content, so
// there's no triple data-load. Vertical scrolling is a different axis, so the
// inner content's own list keeps scrolling up/down.
//
// VOICEOVER path (iOS): that three-page ScrollView is wrong under VoiceOver — a
// three-finger swipe scrolls onto a spacer, which makes iOS announce
// "page 1 of 3 / 3 of 3" and throws focus onto the hidden spacer (out of the
// view). Instead we wrap the content in the native `A11yPagerView`, which
// intercepts the three-finger swipe (`accessibilityScroll:`), pages via onPage,
// and suppresses the page announcement — the screen announces the new period.
// TalkBack (Android) has no such gesture / announcement, so there we just render
// the content directly and page with the toolbar ‹ / › buttons.

export function CalendarPager({
  onPrev,
  onNext,
  children,
}: {
  onPrev: () => void;
  onNext: () => void;
  children: ReactNode;
}) {
  const ref = useRef<ScrollView>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const screenReader = useScreenReaderEnabled();

  if (screenReader) {
    // iOS: the native pager turns a three-finger swipe into onPrev/onNext with
    // no page announcement + no focus loss. Elsewhere, render content directly.
    if (Platform.OS === 'ios') {
      return (
        <A11yPagerView
          style={styles.flex}
          onPage={(e) =>
            e.nativeEvent.direction === 'next' ? onNext() : onPrev()
          }
        >
          {children}
        </A11yPagerView>
      );
    }
    return <View style={styles.flex}>{children}</View>;
  }

  const centerNow = (w: number) => {
    if (w > 0) ref.current?.scrollTo({ x: w, y: 0, animated: false });
  };

  const onLayout = (e: LayoutChangeEvent) => {
    const { width, height } = e.nativeEvent.layout;
    if (width > 0 && (width !== size.width || height !== size.height)) {
      setSize({ width, height });
      // Start centred on the middle (live) page once the page size is known.
      requestAnimationFrame(() => centerNow(width));
    }
  };

  const settle = (e: NativeSyntheticEvent<NativeScrollEvent>) => {
    const w = size.width;
    if (w <= 0) return;
    const x = e.nativeEvent.contentOffset.x;
    if (x <= w * 0.5) {
      onPrev();
      centerNow(w);
    } else if (x >= w * 1.5) {
      onNext();
      centerNow(w);
    }
  };

  const page = { width: size.width, height: size.height };

  return (
    <ScrollView
      ref={ref}
      horizontal
      // NOT pagingEnabled: that puts iOS UIScrollView into paged mode, which
      // makes VoiceOver announce "page X of N" before our month announcement.
      // snapToOffsets + fast deceleration give the same page-snap (and the same
      // three-finger-swipe scroll) WITHOUT the paged trait, so no page noise.
      snapToOffsets={[0, size.width, size.width * 2]}
      decelerationRate="fast"
      disableIntervalMomentum
      showsHorizontalScrollIndicator={false}
      keyboardShouldPersistTaps="handled"
      onLayout={onLayout}
      // A swipe / flick fires momentum; onScrollEndDrag covers a slow drag that
      // stops without momentum. Both settle on a snap offset → onPrev/onNext.
      onMomentumScrollEnd={settle}
      onScrollEndDrag={settle}
      style={styles.flex}
    >
      <View
        style={page}
        accessibilityElementsHidden
        importantForAccessibility="no-hide-descendants"
      />
      <View style={page}>{children}</View>
      <View
        style={page}
        accessibilityElementsHidden
        importantForAccessibility="no-hide-descendants"
      />
    </ScrollView>
  );
}

const styles = StyleSheet.create({ flex: { flex: 1 } });
