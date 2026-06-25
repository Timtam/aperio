import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type { ColorLabel, DayOccurrence, MultiDayInfo } from '@aperio/shared';
import {
  expandAll,
  expandToDayOccurrences,
  localDateKey,
  occurrenceIsoOf,
  seriesIdOf,
} from '@aperio/shared';

import {
  Calendar,
  CalendarEvent,
  getEvents,
  listCalendars,
} from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import { CalendarActions } from '../components/CalendarActions';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { JumpToDateButton } from '../components/JumpToDateButton';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import { useTabBarInset } from '../hooks/useTabBarInset';
import { resolveEventColor } from '../intl/eventColor';
import { useCacheReload } from '../state/cacheObserver';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Accessible Agenda view — a flat ~30-day-forward list of events grouped by
// day, the screen-reader-natural sibling of the day view (EventsScreen). Same
// engine pipeline (listCalendars + palette + getEvents per calendar + expandAll
// recurrence), then expandToDayOccurrences spreads multi-day all-day events
// into one row per covered day (shared with desktop). Each day gets an
// accessible header row; the full day is also folded into every event row's
// label so a row read in isolation still announces its date.

const AGENDA_DAYS = 30;

/** Local-midnight clone of `date`. */
function localMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

function addMonths(date: Date, months: number): Date {
  const next = new Date(date);
  next.setMonth(next.getMonth() + months);
  return next;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function AgendaScreen({
  route,
  navigation,
}: RootStackScreenProps<'Agenda'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();
  const tabBarInset = useTabBarInset();

  // Window anchor (local midnight); seeded from the switcher's `anchor` param so
  // switching Day⇄Agenda keeps the selected date, else today.
  const [anchor, setAnchor] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    return localMidnight(Number.isNaN(seed.getTime()) ? new Date() : seed);
  });
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [occurrences, setOccurrences] = useState<DayOccurrence<CalendarEvent>[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const calendarsById = useMemo(
    () => new Map(calendars.map((c) => [c.id, c])),
    [calendars],
  );
  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );

  // The visible window: anchor 00:00 → end of (anchor + 30 days).
  const range = useMemo(() => {
    const start = localMidnight(anchor);
    const end = addDays(start, AGENDA_DAYS);
    end.setHours(23, 59, 59, 999);
    return { start, end };
  }, [anchor]);

  const fmtFullDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, {
        weekday: 'long',
        year: 'numeric',
        month: 'long',
        day: 'numeric',
      }),
    [i18n.language],
  );
  const fmtShortDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, { year: 'numeric', month: 'long', day: 'numeric' }),
    [i18n.language],
  );
  const timeLabel = useCallback(
    (ev: CalendarEvent): string => {
      if (ev.all_day) return t('views.allDay');
      const fmt = (iso: string) =>
        new Date(iso).toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit' });
      return `${fmt(ev.start)}–${fmt(ev.end)}`;
    },
    [i18n.language, t],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [cals, labels] = await Promise.all([
        listCalendars(),
        listColorLabels().catch(() => [] as ColorLabel[]),
      ]);
      setCalendars(cals);
      setColorLabels(labels);
      const startIso = range.start.toISOString();
      const endIso = range.end.toISOString();
      const perCalendar = await Promise.all(
        cals.map((c) =>
          getEvents({ calendar_id: c.id, start: startIso, end: endIso }).catch(() => []),
        ),
      );
      // Expand recurring series across the whole window first, then spread
      // multi-day all-day events into one occurrence per covered day.
      const expanded = expandAll(perCalendar.flat(), { start: range.start, end: range.end });
      // Drop events from calendars the user hid (the Calendars-screen toggles).
      const visible = expanded.filter((e) => !hidden.has(e.calendar_id));
      setOccurrences(expandToDayOccurrences(visible, range));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setLoading(false);
    }
  }, [announce, range, t, hidden]);

  // Reload when the window changes or the screen regains focus (after the editor).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update while focused when an external calendar-cache refresh lands (the
  // root observer already announced it politely; this just re-reads the window).
  useCacheReload('calendar', load);

  // Per-day event counts for the accessible day-header labels.
  const dayCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const occ of occurrences) {
      const k = localDateKey(occ.day);
      counts.set(k, (counts.get(k) ?? 0) + 1);
    }
    return counts;
  }, [occurrences]);

  const readOnlyIds = useMemo(
    () => new Set(calendars.filter((c) => c.read_only).map((c) => c.id)),
    [calendars],
  );

  // First writable calendar — the seed target for the per-day "+ new event"
  // affordance (the day-anchored create, mirroring the desktop calendar views).
  const firstWritableCalendarId = useMemo(
    () => calendars.find((c) => !c.read_only)?.id ?? null,
    [calendars],
  );
  const addEventOnDay = useCallback(
    (dayKey: string) => {
      if (firstWritableCalendarId == null) return;
      navigation.navigate('EventEditor', {
        eventId: null,
        calendarId: firstWritableCalendarId,
        anchor: dayKey,
      });
    },
    [firstWritableCalendarId, navigation],
  );

  const goToday = useCallback(() => setAnchor(localMidnight(new Date())), []);

  const editEvent = useCallback(
    (ev: CalendarEvent) =>
      navigation.navigate('EventEditor', {
        eventId: seriesIdOf(ev),
        calendarId: ev.calendar_id,
        occurrence: occurrenceIsoOf(ev),
      }),
    [navigation],
  );

  // Move / copy to another calendar — pass the full (possibly expanded) row so
  // the modal can offer the occurrence-vs-series scope.
  const moveCopyEvent = useCallback(
    (ev: CalendarEvent) => navigation.navigate('MoveCopy', { kind: 'event', event: ev }),
    [navigation],
  );

  // Delete with recurrence scope: an occurrence offers "this occurrence" (exdate)
  // vs "whole series"; a single event a plain delete (shared helper).
  const removeEvent = useCallback(
    (ev: CalendarEvent) =>
      confirmDeleteEvent(
        ev,
        t,
        (message) => {
          announce(message);
          void load();
        },
        (message) => {
          setError(message);
          announce(t('mobile.error', { message }));
        },
      ),
    [announce, load, t],
  );

  const rowLabel = useCallback(
    (ev: CalendarEvent, day: Date, span: MultiDayInfo | null): string => {
      let label = t('views.agenda.eventLabel', {
        day: fmtFullDate(day),
        title: ev.title,
        time: timeLabel(ev),
        calendar: calendarsById.get(ev.calendar_id)?.name ?? '—',
      });
      if (span) {
        label += t('views.multiDaySuffix', { day: span.dayIndex, total: span.totalDays });
      }
      const colour = resolveEventColor(ev, calendarsById, labelsById);
      if (colour.labelName) {
        label += t('mobile.colorLabelSuffix', { name: colour.labelName });
      }
      return label;
    },
    [calendarsById, fmtFullDate, labelsById, t, timeLabel],
  );

  return (
    <View style={styles.screen}>
      {/* Day ⇄ Week ⇄ Month ⇄ Agenda, carrying the anchor so the date survives
          the switch. replace (not push): sibling views swap in place, keeping
          the stack flat. Pressing the active view is suppressed by the switcher. */}
      <CalendarViewSwitcher
        active="agenda"
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: anchor.toISOString() })
        }
      />

      <View style={styles.navBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.prev')}
          onPress={() => setAnchor((a) => localMidnight(addMonths(a, -1)))}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText} importantForAccessibility="no">‹</Text>
        </Pressable>
        <Text style={styles.rangeHeading} accessibilityRole="header">
          {`${fmtShortDate(range.start)} – ${fmtShortDate(localMidnight(addDays(anchor, AGENDA_DAYS)))}`}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('toolbar.next')}
          onPress={() => setAnchor((a) => localMidnight(addMonths(a, 1)))}
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
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.today')}</Text>
        </Pressable>
        {/* A real button that opens the picker on tap (no always-visible date
            chip duplicating the heading). */}
        <JumpToDateButton
          value={anchor}
          onSelect={(date) => {
            setError(null);
            setAnchor(localMidnight(date));
          }}
        />
        <CalendarActions navigation={navigation} anchorDay={anchor} />
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('views.loading')}>
          {t('views.loading')}
        </Text>
      ) : occurrences.length === 0 ? (
        <Text style={styles.muted}>{t('views.agenda.empty')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          accessibilityLabel={t('views.agenda.eventList')}
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
        >
          {(() => {
            let prevKey: string | null = null;
            let prevDay: Date | null = null;
            const rows: ReactNode[] = [];
            // Close the current day group with a "+ new event" affordance seeded
            // to that day (only when a writable calendar exists to host it).
            const pushDayFooter = () => {
              if (prevKey == null || prevDay == null || firstWritableCalendarId == null) return;
              const dayKey = prevKey;
              const day = prevDay;
              rows.push(
                <Pressable
                  key={`add-${dayKey}`}
                  accessibilityRole="button"
                  accessibilityLabel={`${t('toolbar.newEvent')}, ${fmtFullDate(day)}`}
                  onPress={() => addEventOnDay(dayKey)}
                  style={({ pressed }) => [styles.newEventButton, pressed && styles.pressed]}
                >
                  <Text style={styles.newEventText}>{t('toolbar.newEvent')}</Text>
                </Pressable>,
              );
            };
            for (const occ of occurrences) {
              const key = localDateKey(occ.day);
              if (key !== prevKey) {
                pushDayFooter();
                prevKey = key;
                prevDay = occ.day;
                rows.push(
                  <Text
                    key={`h-${key}`}
                    accessibilityRole="header"
                    accessibilityLabel={t('views.agenda.dayLabel', {
                      day: fmtFullDate(occ.day),
                      count: dayCounts.get(key) ?? 0,
                    })}
                    style={styles.dayHeader}
                  >
                    {fmtFullDate(occ.day)}
                  </Text>,
                );
              }
              rows.push(renderRow(occ, key));
            }
            pushDayFooter();
            return rows;
          })()}
        </ScrollView>
      )}
    </View>
  );

  function renderRow(occ: DayOccurrence<CalendarEvent>, dayKey: string) {
    const ev = occ.ev;
    const rowKey = `${ev.id}@${dayKey}`;
    const hex = resolveEventColor(ev, calendarsById, labelsById).hex;
    const dot =
      hex != null ? (
        <View
          accessible={false}
          importantForAccessibility="no"
          style={[styles.colorDot, { backgroundColor: hex }]}
        />
      ) : null;
    const badge = occ.span
      ? ` ${t('views.multiDayCompact', { day: occ.span.dayIndex, total: occ.span.totalDays })}`
      : '';
    if (readOnlyIds.has(ev.calendar_id)) {
      return (
        <View
          key={rowKey}
          accessible
          accessibilityRole="text"
          accessibilityLabel={rowLabel(ev, occ.day, occ.span)}
          style={styles.row}
        >
          {dot}
          <View style={styles.rowText}>
            <Text style={styles.eventTitle} importantForAccessibility="no">
              {ev.title}
              {badge}
            </Text>
            <Text style={styles.eventTime} importantForAccessibility="no">
              {timeLabel(ev)}
            </Text>
          </View>
        </View>
      );
    }
    return (
      <View
        key={rowKey}
        accessible
        accessibilityRole="button"
        accessibilityLabel={rowLabel(ev, occ.day, occ.span)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={[
          { name: 'activate', label: t('mobile.editTaskLabel') },
          { name: 'moveCopy', label: t('mobile.moveCopy') },
          { name: 'delete', label: t('dialogs.event.delete') },
        ]}
        onAccessibilityAction={(e) => {
          if (e.nativeEvent.actionName === 'delete') removeEvent(ev);
          else if (e.nativeEvent.actionName === 'moveCopy') moveCopyEvent(ev);
          else editEvent(ev);
        }}
        style={styles.row}
      >
        {dot}
        <Pressable accessible={false} onPress={() => editEvent(ev)} style={styles.rowText}>
          <Text style={styles.eventTitle} importantForAccessibility="no">
            {ev.title}
            {badge}
          </Text>
          <Text style={styles.eventTime} importantForAccessibility="no">
            {timeLabel(ev)}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('dialogs.event.delete')}: ${ev.title}`}
          onPress={() => removeEvent(ev)}
          style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
        >
          <Text style={styles.deleteButtonText}>{t('dialogs.event.delete')}</Text>
        </Pressable>
      </View>
    );
  }
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
      fontSize: 16,
      fontWeight: '700',
      color: c.textPrimary,
      textAlign: 'center',
    },
    navButton: {
      width: 48,
      height: 48,
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
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    list: { gap: 10, padding: 16 },
    dayHeader: {
      fontSize: 15,
      fontWeight: '700',
      color: c.textLabel,
      marginTop: 8,
      marginBottom: 2,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowText: { flex: 1, gap: 2 },
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    eventTitle: { fontSize: 18, fontWeight: '600', color: c.textPrimary },
    eventTime: { fontSize: 14, color: c.textSecondary },
    deleteButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    deleteButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    newEventButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    newEventText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
