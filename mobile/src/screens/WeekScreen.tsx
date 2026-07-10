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
import { readWeekStart, type WeekStart } from '../settings/weekStart';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome } from '../theme/uiScale';

// Accessible Week view — the screen-reader-first port of the desktop WeekView.
// The desktop's 7-column aria-activedescendant grid has no TalkBack/VoiceOver
// analogue; the faithful mobile equivalent is the shared linear CalendarDayList
// (one accessible section per day). This screen owns only the week chrome: the
// Day/Week/Month/Agenda switcher, the ISO-week header + prev/next-week nav, and
// today / jump-to-date. The week starts on the synced `view.weekStart` pref
// (Monday by default); the header shows the ISO-8601 week number (always
// Monday-based, regardless of the visual start day).

/** Local-midnight clone of `date`. */
function localMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

/** Local-midnight start of `date`'s week for the given visual start day —
 *  matches date-fns `startOfWeek(date, { weekStartsOn })` (the desktop's). */
function startOfWeekLocal(date: Date, weekStartsOn: WeekStart): Date {
  const d = localMidnight(date);
  const diff = (d.getDay() - weekStartsOn + 7) % 7;
  d.setDate(d.getDate() - diff);
  return d;
}

/** Standard ISO-8601 week number (1–53) — the week is the one containing its
 *  Thursday; week 1 is the week with the year's first Thursday. */
function isoWeekNumber(date: Date): number {
  const d = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  const dayNum = (d.getUTCDay() + 6) % 7; // Mon=0 … Sun=6
  d.setUTCDate(d.getUTCDate() - dayNum + 3); // Thursday of this ISO week
  const firstThursday = new Date(Date.UTC(d.getUTCFullYear(), 0, 4));
  const firstDayNum = (firstThursday.getUTCDay() + 6) % 7;
  firstThursday.setUTCDate(firstThursday.getUTCDate() - firstDayNum + 3);
  return 1 + Math.round((d.getTime() - firstThursday.getTime()) / 604800000);
}

export default function WeekScreen({ navigation, route }: RootStackScreenProps<'Week'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [anchor, setAnchor] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    return localMidnight(Number.isNaN(seed.getTime()) ? new Date() : seed);
  });
  const [weekStart, setWeekStart] = useState<WeekStart>(1);

  // Read the synced week-start pref on mount + whenever the screen regains focus
  // (it can change in Settings on this or another device).
  useEffect(() => {
    const read = () => void readWeekStart().then(setWeekStart);
    const unsubscribe = navigation.addListener('focus', read);
    read();
    return unsubscribe;
  }, [navigation]);

  // The seven visible days (local midnights) + the fetch range covering them.
  const { days, range } = useMemo(() => {
    const start = startOfWeekLocal(anchor, weekStart);
    const ds = Array.from({ length: 7 }, (_, i) => addDays(start, i));
    const end = new Date(ds[6]);
    end.setHours(23, 59, 59, 999);
    return { days: ds, range: { start: ds[0], end } };
  }, [anchor, weekStart]);

  const fmtShortDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, { year: 'numeric', month: 'long', day: 'numeric' }),
    [i18n.language],
  );

  // ISO week of the visual week's 4th day — Monday-based regardless of the
  // visual start (matches the desktop header).
  const isoWeek = useMemo(() => isoWeekNumber(addDays(days[0], 3)), [days]);

  const weekLabel = useMemo(
    () =>
      `${t('views.week.kw', { week: isoWeek })} · ${fmtShortDate(days[0])} – ${fmtShortDate(days[6])}`,
    [days, fmtShortDate, isoWeek, t],
  );

  // Announce the period on navigation (the three-finger swipe / prev-next change
  // the week silently otherwise). Keyed on the ANCHOR, not the label: the synced
  // week-start pref hydrates ASYNC after mount and recomputes the label, and a
  // label-keyed skip-first-render guard would then announce on plain screen
  // OPEN for every non-Monday-start user (racing VoiceOver's own screen-change
  // announcement). Only user navigation changes the anchor.
  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );
  // TRIGGER on the anchor changing (only user navigation touches it — the
  // async hydration only shifts days[0], so it can never announce), FILTER on
  // the week actually changing (a jump to another day of the SAME week stays
  // silent, like MonthScreen's label-keyed announce).
  const lastAnchor = useRef(anchor.getTime());
  const lastWeekStart = useRef(days[0].getTime());
  useEffect(() => {
    const weekStart = days[0].getTime();
    const anchorChanged = anchor.getTime() !== lastAnchor.current;
    const weekChanged = weekStart !== lastWeekStart.current;
    lastAnchor.current = anchor.getTime();
    lastWeekStart.current = weekStart;
    if (anchorChanged && weekChanged) announce(weekLabel);
  }, [anchor, days, weekLabel, announce]);

  const goToday = useCallback(() => setAnchor(localMidnight(new Date())), []);

  const stepWeek = useCallback(
    (delta: number) => setAnchor((a) => localMidnight(addDays(a, delta * 7))),
    [],
  );

  // VoiceOver MAGIC TAP (two-finger double-tap) creates a new event on the
  // week's anchor day — the same flow as the toolbar's New Event.
  const { addEvent: magicTapCreate } = useNewEventOnDay(navigation, anchor);

  return (
    <View style={styles.screen} onMagicTap={magicTapCreate}>
      <CalendarViewSwitcher
        active="week"
        // replace (not push): sibling views swap in place, keeping the stack
        // flat; the anchor rides along so the date survives the switch.
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: anchor.toISOString() })
        }
      />

      <View style={styles.navBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.prev')}
          onPress={() => stepWeek(-1)}
          hitSlop={8}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">‹</Text>
        </Pressable>
        <Text style={styles.rangeHeading} accessibilityRole="header">
          {weekLabel}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.next')}
          onPress={() => stepWeek(1)}
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
        {/* A real button that opens the picker on tap (any day selects its week). */}
        <JumpToDateButton value={anchor} onSelect={(date) => setAnchor(localMidnight(date))} />
        <CalendarActions navigation={navigation} anchorDay={anchor} />
      </View>

      {/* Three-finger swipe (VoiceOver) / horizontal flick pages between weeks;
          vertical scrolling stays with the day list. Mirrors MonthScreen. */}
      <CalendarPager onPrev={() => stepWeek(-1)} onNext={() => stepWeek(1)}>
        <CalendarDayList
          navigation={navigation}
          days={days}
          range={range}
          gridLabel={t('views.week.gridLabel')}
          emptyText={t('views.week.empty')}
          dayAnnounceKey="views.week.dayAnnounce"
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
