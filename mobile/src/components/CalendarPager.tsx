import { type ReactNode, useRef, useState } from 'react';
import {
  type LayoutChangeEvent,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';

// Horizontal swipe-to-page wrapper for the calendar views — gives VoiceOver's
// three-finger swipe (the system scroll gesture, which Apple Calendar maps to
// month/week/day paging) something to page, and lets sighted users flick
// horizontally too.
//
// It's a pagingEnabled horizontal ScrollView with THREE full-size pages: empty
// spacers either side of the live content (the middle). A horizontal swipe
// lands on a spacer → we shift the period (onPrev / onNext) and snap straight
// back to the middle, so paging feels "infinite". Only the middle renders
// content, so there's no triple data-load; the spacers are hidden from the
// reader. Vertical scrolling is a different axis, so the inner content's own
// list keeps scrolling up/down.

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
      pagingEnabled
      showsHorizontalScrollIndicator={false}
      keyboardShouldPersistTaps="handled"
      onLayout={onLayout}
      // VoiceOver paging animates to the page (fires momentum); a flick fires
      // momentum too. onScrollEndDrag covers a slow drag that stops without it.
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
