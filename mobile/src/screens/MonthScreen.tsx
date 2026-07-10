import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import { CalendarActions } from '../components/CalendarActions';
import { useNewEventOnDay } from '../components/useNewEventOnDay';
import { CalendarDayList } from '../components/CalendarDayList';
import { CalendarPager } from '../components/CalendarPager';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { JumpToDateButton } from '../components/JumpToDateButton';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome } from '../theme/uiScale';

// Accessible Month view — the screen-reader-first port of the desktop MonthView.
// The desktop's 6-week grid is a visual layout; the faithful SR equivalent is
// the shared linear CalendarDayList scoped to the calendar month (the 1st to the
// last, no adjacent-month padding — padding only serves a visual grid). This
// screen owns the month chrome: the Day/Week/Month/Agenda switcher, the month
// header + prev/next-month nav, and today / jump-to-date. (Unlike Week, the
// month-days list needs no week-start — there's no week grouping or padding.)

/** Local-midnight clone of `date`. */
function localMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export default function MonthScreen({ navigation, route }: RootStackScreenProps<'Month'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [anchor, setAnchor] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    return localMidnight(Number.isNaN(seed.getTime()) ? new Date() : seed);
  });
  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // Every day of the anchor's calendar month (1st → last) + the covering range.
  const { days, range } = useMemo(() => {
    const y = anchor.getFullYear();
    const m = anchor.getMonth();
    const daysInMonth = new Date(y, m + 1, 0).getDate();
    const ds = Array.from({ length: daysInMonth }, (_, i) => new Date(y, m, i + 1));
    const end = new Date(y, m, daysInMonth, 23, 59, 59, 999);
    return { days: ds, range: { start: new Date(y, m, 1), end } };
  }, [anchor]);

  const monthLabel = useMemo(
    () => anchor.toLocaleDateString(i18n.language, { month: 'long', year: 'numeric' }),
    [anchor, i18n.language],
  );

  // Announce the period on navigation (the three-finger swipe / prev-next change
  // the month silently otherwise). Skip the first render — nothing changed yet.
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    announce(monthLabel);
  }, [monthLabel, announce]);

  const goToday = useCallback(() => setAnchor(localMidnight(new Date())), []);

  const stepMonth = useCallback(
    (delta: number) =>
      setAnchor((a) => new Date(a.getFullYear(), a.getMonth() + delta, 1)),
    [],
  );

  // VoiceOver MAGIC TAP (two-finger double-tap) creates a new event on the
  // month's anchor day — the same flow as the toolbar's New Event.
  const { addEvent: magicTapCreate } = useNewEventOnDay(navigation, anchor);

  return (
    <View style={styles.screen} onMagicTap={magicTapCreate}>
      <CalendarViewSwitcher
        active="month"
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: anchor.toISOString() })
        }
      />

      <View style={styles.navBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.prev')}
          onPress={() => stepMonth(-1)}
          hitSlop={8}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">‹</Text>
        </Pressable>
        <Text style={styles.rangeHeading} accessibilityRole="header">
          {monthLabel}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.next')}
          onPress={() => stepMonth(1)}
          hitSlop={8}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">›</Text>
        </Pressable>
      </View>

      <View style={styles.actionBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.today')}
          onPress={goToday}
          hitSlop={8}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.today')}</Text>
        </Pressable>
        {/* A real button that opens the picker on tap (any day in a month
            selects that month). */}
        <JumpToDateButton value={anchor} onSelect={(date) => setAnchor(localMidnight(date))} />
        <CalendarActions navigation={navigation} anchorDay={anchor} />
      </View>

      {/* Three-finger swipe (VoiceOver) / horizontal flick pages between months;
          vertical scrolling stays with the day list. */}
      <CalendarPager onPrev={() => stepMonth(-1)} onNext={() => stepMonth(1)}>
        <CalendarDayList
          navigation={navigation}
          days={days}
          range={range}
          gridLabel={t('views.month.gridLabel')}
          emptyText={t('views.month.empty')}
          dayAnnounceKey="views.month.dayAnnounce"
        />
      </CalendarPager>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    navBar: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingHorizontal: 12,
      paddingTop: 12,
    },
    rangeHeading: {
      flex: 1,
      fontSize: 15,
      fontWeight: '700',
      color: c.textPrimary,
      textAlign: 'center',
    },
    // Hug the chevron instead of a fixed 44×44 box (which left a big empty
    // square around a narrow glyph, independent of font size). The 44pt tap
    // target is preserved by `hitSlop` on the Pressable.
    navButton: {
      paddingVertical: chrome(4),
      paddingHorizontal: chrome(12),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: c.surfaceAlt,
    },
    navButtonText: { fontSize: 24, color: c.textPrimary, lineHeight: 26 },
    actionBar: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, padding: 12, alignItems: 'center' },
    ghostButton: {
      paddingVertical: chrome(6),
      paddingHorizontal: chrome(12),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
  });
