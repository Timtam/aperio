import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import { expandAll } from '@aperio/shared';

import { getEvents, listCalendars } from '../api/calendar';
import { CalendarActions } from '../components/CalendarActions';
import { CalendarPager } from '../components/CalendarPager';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import { useTabBarInset } from '../hooks/useTabBarInset';
import type { RootStackScreenProps } from '../navigation/types';
import { useCacheReload } from '../state/cacheObserver';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome, chromeTouch } from '../theme/uiScale';

// Accessible Year view — the screen-reader-first port of the desktop YearView.
// The desktop's 12×31 mini-grid is a purely visual navigation aid; the faithful
// SR equivalent is a LINEAR list of the 12 months, each a button announcing its
// name + event count, that opens the Month view for that month. Reuses the
// shared event pipeline (per-calendar getEvents over the year + expandAll, so
// recurring series count per occurrence). Events only — the year overview's
// natural granularity (tasks have their own screen). Both audiences: a visible
// count for sighted users, the full count folded into each button's a11y label.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function YearScreen({ navigation, route }: RootStackScreenProps<'Year'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();
  const tabBarInset = useTabBarInset();

  const [year, setYear] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    return (Number.isNaN(seed.getTime()) ? new Date() : seed).getFullYear();
  });
  const [counts, setCounts] = useState<number[]>(() => Array<number>(12).fill(0));
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // Request-epoch guard: the latest load wins (a year step re-fires load while an
  // earlier fetch may still be in flight). Same precedent as CalendarDayList.
  const reqToken = useRef(0);
  const load = useCallback(async () => {
    const token = (reqToken.current += 1);
    setLoading(true);
    setError(null);
    try {
      // listCalendars primes the Host route map (getEvents routes by calendar id).
      const cals = await listCalendars();
      const start = new Date(year, 0, 1);
      const end = new Date(year, 11, 31, 23, 59, 59, 999);
      const startIso = start.toISOString();
      const endIso = end.toISOString();
      const perCalendar = await Promise.all(
        cals.map((c) =>
          getEvents({ calendar_id: c.id, start: startIso, end: endIso }).catch(() => []),
        ),
      );
      if (reqToken.current !== token) return;
      // Expand recurring series so each occurrence counts in its own month.
      const expanded = expandAll(perCalendar.flat(), { start, end });
      const next = Array<number>(12).fill(0);
      for (const ev of expanded) {
        if (hidden.has(ev.calendar_id)) continue;
        const d = new Date(ev.start);
        if (d.getFullYear() === year) {
          const m = d.getMonth();
          next[m] = (next[m] ?? 0) + 1;
        }
      }
      setCounts(next);
    } catch (err) {
      if (reqToken.current !== token) return;
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      if (reqToken.current === token) setLoading(false);
    }
  }, [announce, t, year, hidden]);

  // Reload when the year changes or the screen regains focus (after a drill-in).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update on an external calendar-cache refresh (politely announced by the
  // root observer); the same load recomputes the per-month counts.
  useCacheReload('calendar', load);

  const months = useMemo(() => {
    const fmt = new Intl.DateTimeFormat(i18n.language, { month: 'long', year: 'numeric' });
    return Array.from({ length: 12 }, (_, m) => ({
      index: m,
      label: fmt.format(new Date(year, m, 1)),
    }));
  }, [i18n.language, year]);

  const openMonth = useCallback(
    (m: number) => navigation.replace('Month', { anchor: new Date(year, m, 1).toISOString() }),
    [navigation, year],
  );

  // Announce the period on navigation (the three-finger swipe / prev-next change
  // the year silently otherwise). Skip the first render — nothing changed yet.
  // Mirrors MonthScreen.
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    announce(String(year));
  }, [year, announce]);

  return (
    <View style={styles.screen}>
      <CalendarViewSwitcher
        active="year"
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], {
            anchor: new Date(year, 0, 1).toISOString(),
          })
        }
      />

      <View style={styles.navBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.prev')}
          onPress={() => setYear((y) => y - 1)}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">
            ‹
          </Text>
        </Pressable>
        <Text style={styles.rangeHeading} accessibilityRole="header">
          {String(year)}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.next')}
          onPress={() => setYear((y) => y + 1)}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">
            ›
          </Text>
        </Pressable>
      </View>

      <View style={styles.actionBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.today')}
          onPress={() => setYear(new Date().getFullYear())}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.today')}</Text>
        </Pressable>
        <CalendarActions navigation={navigation} anchorDay={new Date(year, 0, 1)} />
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {/* Three-finger swipe (VoiceOver) / horizontal flick pages between years;
          vertical scrolling stays with the month list. Mirrors MonthScreen. */}
      <CalendarPager onPrev={() => setYear((y) => y - 1)} onNext={() => setYear((y) => y + 1)}>
        <ScrollView
          accessibilityRole="list"
          accessibilityLabel={t('views.year.gridLabel')}
          // flex:1 so the list fills the pager's fixed-size page and keeps its
          // own vertical scrolling (the CalendarDayList precedent).
          style={styles.scroll}
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
        >
          {months.map((mo) => (
            <Pressable
              key={mo.index}
              accessibilityRole="button"
              accessibilityLabel={t('views.year.monthAnnounce', {
                month: mo.label,
                count: counts[mo.index] ?? 0,
              })}
              onPress={() => openMonth(mo.index)}
              style={({ pressed }) => [styles.monthRow, pressed && styles.pressed]}
            >
              <Text style={styles.monthName} importantForAccessibility="no">
                {mo.label}
              </Text>
              <Text style={styles.monthCount} importantForAccessibility="no">
                {loading ? '…' : String(counts[mo.index] ?? 0)}
              </Text>
            </Pressable>
          ))}
        </ScrollView>
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
    actionBar: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, padding: 12, alignItems: 'center' },
    ghostButton: {
      paddingVertical: chrome(10),
      paddingHorizontal: chrome(13),
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    scroll: { flex: 1 },
    list: { gap: chrome(8), padding: chrome(12) },
    monthRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: chrome(10),
      padding: chrome(12),
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    monthName: { flex: 1, fontSize: 18, fontWeight: '600', color: c.textPrimary },
    monthCount: { fontSize: 16, fontWeight: '700', color: c.textSecondary, minWidth: 28, textAlign: 'right' },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
    pressed: { opacity: 0.7 },
  });
