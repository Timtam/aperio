import { useRoute } from '@react-navigation/native';
import { type ReactNode, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  type LayoutChangeEvent,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  Platform,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import { A11yPagerView } from '../../modules/a11y-pager';
import { registerPageAction } from '../a11y/gestureHost';
import { takePeriodAnnounceOverride } from './periodAnnounce';
import { useScreenReaderEnabled } from '../a11y/useScreenReaderEnabled';
import { useThemedStyles, type ThemeColors } from '../theme';

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
// and suppresses the page announcement.
//
// Under VoiceOver we ALSO render a stable, focusable PERIOD HEADER at the top of
// the pager, and after each page we move VoiceOver focus onto it. This is the
// fix for two problems the raw pager had:
//   1. On a period change the previously-focused chip unmounts, so VoiceOver's
//      focus fell OUT of the pager onto the screen's heading (which sits above
//      the pager) — and a three-finger swipe there no longer bubbles to the
//      pager, so paging silently stopped working until the user navigated back
//      in by hand.
//   2. If the next period's data wasn't cached yet, there was briefly NO stable
//      element inside the pager to land on, so VoiceOver "blocked".
// The header is chrome (no data dependency, always mounted), lives INSIDE the
// pager (so a swipe from it keeps paging), and announces the new period on each
// flip. The screen therefore hides its own visual heading under this path (see
// `pagerOwnsHeading` in the calendar screens) so there's exactly one heading.
//
// TalkBack (Android) has no such gesture / announcement, so there we just render
// the content directly and page with the toolbar ‹ / › buttons.
//
// `useCalendarPagerOwnsHeading` (its own file, for fast-refresh) tells the
// screens when this component owns the heading so they drop theirs.

export function CalendarPager({
  onPrev,
  onNext,
  periodLabel,
  children,
}: {
  onPrev: () => void;
  onNext: () => void;
  /** Current period, e.g. "Week of 3 March" — shown + focused as the pager's
   *  own header under VoiceOver so focus never leaves the pager. */
  periodLabel: string;
  children: ReactNode;
}) {
  const ref = useRef<ScrollView>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const screenReader = useScreenReaderEnabled();
  const styles = useThemedStyles(makeStyles);

  // The pager renders its own visible period header under VoiceOver (the screens
  // drop theirs via `useCalendarPagerOwnsHeading`), announced on each change.
  const firstRun = useRef(true);
  useEffect(() => {
    // Only the native VoiceOver pager announces the period.
    if (!(screenReader && Platform.OS === 'ios')) return;
    if (firstRun.current) {
      // Don't announce the initial period on mount — VoiceOver is already
      // reading the freshly-shown screen.
      firstRun.current = false;
      return;
    }
    // Announce the new period on ANY change — a three-finger swipe (in-content or
    // window-routed from chrome) or a toolbar ‹ ›/Today/jump — promptly and
    // INTERRUPTINGLY. `queue: false` cuts through so the range is spoken
    // immediately instead of being dropped behind, or delayed seconds by, the day
    // list's reload accessibility churn (the reported ~3 s delay). We deliberately
    // no longer move VoiceOver focus onto the header after a swipe: that focus
    // move was exactly what the reload delayed, and the window-level scroll
    // fallback (modules/a11y-gestures) keeps paging working even if the cursor
    // drifts off the pager.
    //
    // A programmatic jump ("Go to the current task") pre-registers a richer
    // label naming the exact target day — the bare week/month name would cut
    // off the jump's own announcement and leave the user without the one fact
    // the action exists to deliver. Consume it here so the interrupting
    // utterance IS the informative one.
    AccessibilityInfo.announceForAccessibilityWithOptions(
      takePeriodAnnounceOverride() ?? periodLabel,
      { queue: false },
    );
  }, [periodLabel, screenReader]);

  // Register this screen's pager step under its route name so a VoiceOver
  // three-finger swipe made when focus is on chrome (a heading / the view
  // switcher / the ‹ › buttons — outside the in-content A11yPagerView) still
  // pages: the window-level native catcher (modules/a11y-gestures) routes such
  // swipes to the active route's handler. Registered on mount, cleared on unmount
  // — routing by the live active route is robust to the native tab bar's
  // unreliable focus events.
  const route = useRoute();
  useEffect(
    () =>
      registerPageAction(route.name, (direction) => {
        if (direction === 'next') onNext();
        else onPrev();
      }),
    [route.name, onNext, onPrev],
  );

  if (screenReader) {
    // iOS: the native pager turns a three-finger swipe into onPrev/onNext with
    // no page announcement; the header below keeps focus inside the pager.
    if (Platform.OS === 'ios') {
      return (
        <A11yPagerView
          style={styles.flex}
          onPage={(e) => {
            if (e.nativeEvent.direction === 'next') onNext();
            else onPrev();
          }}
        >
          <Text accessibilityRole="header" style={styles.pagerHeader}>
            {periodLabel}
          </Text>
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

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    flex: { flex: 1 },
    // The VoiceOver-only period header. Visible (the screen drops its own
    // heading under this path) + styled like the screens' range heading.
    pagerHeader: {
      fontSize: 17,
      fontWeight: '700',
      color: c.textPrimary,
      paddingHorizontal: 16,
      paddingVertical: 8,
    },
  });
