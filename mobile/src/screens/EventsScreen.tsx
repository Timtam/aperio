import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from 'react-native';

import { CalendarActions } from '../components/CalendarActions';
import { CalendarDayList } from '../components/CalendarDayList';
import { CalendarPager } from '../components/CalendarPager';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { JumpToDateButton } from '../components/JumpToDateButton';
import { SegmentedSelect } from '../components/SegmentedSelect';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import type { RootStackScreenProps } from '../navigation/types';
import { readTaskBehaviour, writeDayViewMode } from '../state/taskBehaviour';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome, chromeTouch } from '../theme/uiScale';

// Accessible day view — the screen-reader-first equivalent of the desktop
// DayView. It owns only the day chrome: the Day/Week/Month/Agenda switcher,
// previous/next-day navigation, today + jump-to-date, and the shared calendar
// actions (Calendars / Search / New event). The day's CONTENT — events AND
// tasks for the day, merged chronologically with recurring series expanded — is
// rendered by the shared CalendarDayList engine (the same one behind the Week
// and Month views). That's the fix for "tasks don't show in the day view": the
// old bespoke list fetched events only. The list's own per-day header is
// suppressed (showDayHeaders={false}) because the date already shows in the nav
// bar — a second date heading would just clutter screen-reader heading nav.

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

/** Local-midnight clone of `date`. */
function localMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export default function EventsScreen({ navigation, route }: RootStackScreenProps<'Events'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  // The selected day at local midnight (date-only semantics for the heading).
  // Seeded from the view-switcher's `anchor` param so switching keeps the date,
  // else today.
  const [day, setDay] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    const base = Number.isNaN(seed.getTime()) ? new Date() : seed;
    return localMidnight(base);
  });

  // The single visible day + the instant range covering it; CalendarDayList
  // loads + expands the day's events and tasks within this window.
  const { days, range } = useMemo(() => {
    const start = localMidnight(day);
    const end = new Date(day.getFullYear(), day.getMonth(), day.getDate(), 23, 59, 59, 999);
    return { days: [start], range: { start, end } };
  }, [day]);

  const dayLabel = day.toLocaleDateString(i18n.language, {
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });

  // Announce the period on navigation (the three-finger swipe / prev-next change
  // the day silently otherwise). Skip the first render — nothing changed yet.
  // Mirrors MonthScreen.
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    AccessibilityInfo.announceForAccessibility(dayLabel);
  }, [dayLabel]);

  const goToday = useCallback(() => setDay(localMidnight(new Date())), []);

  const stepDay = useCallback(
    (delta: number) => setDay((d) => addDays(d, delta)),
    [],
  );

  // The synced single-day layout (`calendar.dayViewMode`, default 'grid'):
  // proportional hour-grid vs. compact list. Hydrated on mount and re-read on
  // focus so a Settings change or a peer-device sync reflects without a restart
  // (the same pattern CalendarDayList uses for the effort-sizing pref). The
  // toolbar segmented control writes it + flips local state immediately.
  const [dayViewMode, setDayViewMode] = useState<'grid' | 'list'>('grid');
  useEffect(() => {
    const read = () => void readTaskBehaviour().then((b) => setDayViewMode(b.dayViewMode));
    read();
    const unsubscribe = navigation.addListener('focus', read);
    return unsubscribe;
  }, [navigation]);
  const onSelectDayViewMode = useCallback((next: 'grid' | 'list') => {
    setDayViewMode(next);
    void writeDayViewMode(next);
  }, []);
  const dayViewModeOptions = useMemo(
    () => [
      { value: 'grid' as const, label: t('toolbar.dayViewMode.grid') },
      { value: 'list' as const, label: t('toolbar.dayViewMode.list') },
    ],
    [t],
  );

  return (
    <View style={styles.screen}>
      <CalendarViewSwitcher
        active="day"
        // replace (not push): the calendar views are siblings, so swap in place
        // — keeps the stack flat while the fresh mount still picks up the anchor
        // date from params.
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: day.toISOString() })
        }
      />

      {/* Day navigation */}
      <View style={styles.dayBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.prevDay')}
          onPress={() => stepDay(-1)}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">
            ‹
          </Text>
        </Pressable>
        <Text style={styles.dayHeading} accessibilityRole="header">
          {dayLabel}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.nextDay')}
          onPress={() => stepDay(1)}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">
            ›
          </Text>
        </Pressable>
      </View>

      {/* Jump straight to a date — a real button that opens the picker on tap. */}
      <View style={styles.jumpBar}>
        <JumpToDateButton value={day} onSelect={(date) => setDay(localMidnight(date))} />
      </View>

      {/* Day-layout quick-toggle (Stundenraster / Liste) — SegmentedSelect:
          a visible legend Text (sighted users see a labelled toggle) plus the
          native SegmentedControl (real UISegmentedControl on iOS, Material
          segmented button row on Android, so VoiceOver/TalkBack get native
          segmented semantics with the legend announced). Writes the synced
          `calendar.dayViewMode` pref + flips local state immediately. Single-day
          only (this screen IS the single day). */}
      <View style={styles.modeBar}>
        <SegmentedSelect
          label={t('toolbar.dayViewMode.label')}
          value={dayViewMode}
          options={dayViewModeOptions}
          onChange={onSelectDayViewMode}
        />
      </View>

      <View style={styles.actionBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.today')}
          onPress={goToday}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.today')}</Text>
        </Pressable>
        <CalendarActions navigation={navigation} anchorDay={day} />
      </View>

      {/* Three-finger swipe (VoiceOver) / horizontal flick pages between days;
          vertical scrolling stays with the day list. Mirrors MonthScreen. */}
      <CalendarPager onPrev={() => stepDay(-1)} onNext={() => stepDay(1)}>
        <CalendarDayList
          navigation={navigation}
          days={days}
          range={range}
          gridLabel={t('views.day.gridLabel')}
          emptyText={t('views.day.empty')}
          dayAnnounceKey="views.day.dayAnnounce"
          showDayHeaders={false}
          // Single-day view → the synced `calendar.dayViewMode`: 'grid' = the
          // proportional 24h hour-grid (events placed by start, sized by
          // duration), 'list' = the compact chronological list (event blocks sized
          // by duration, tasks by effort). Visual only; the list semantics are
          // unchanged. Week/Month/Agenda render CalendarDayList without this prop
          // (plain linear list).
          dayLayout={dayViewMode}
        />
      </CalendarPager>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    dayBar: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingHorizontal: 12,
      paddingTop: 14,
    },
    dayHeading: {
      flex: 1,
      fontSize: 16,
      fontWeight: '700',
      color: c.textPrimary,
      textAlign: 'center',
    },
    navButton: {
      width: chromeTouch(44),
      height: chromeTouch(44),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: c.surfaceAlt,
    },
    navButtonText: { fontSize: 26, color: c.textPrimary, lineHeight: 30 },
    // Wraps the jump-to-date button with the screen's horizontal padding;
    // flex-start keeps it content-width on the left.
    jumpBar: {
      paddingHorizontal: 12,
      paddingTop: 12,
      alignItems: 'flex-start',
    },
    modeBar: { paddingHorizontal: 12, paddingTop: 12 },
    actionBar: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 10,
      padding: 12,
      alignItems: 'center',
    },
    ghostButton: {
      paddingVertical: chrome(10),
      paddingHorizontal: chrome(13),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
  });
