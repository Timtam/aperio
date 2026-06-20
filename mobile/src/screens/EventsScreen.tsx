import { DateTimePicker } from '@expo/ui/community/datetime-picker';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type { ColorLabel } from '@aperio/shared';
import { expandAll, localDateKey, occurrenceIsoOf, seriesIdOf } from '@aperio/shared';

import {
  Calendar,
  CalendarEvent,
  getEvents,
  listCalendars,
} from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import { CalendarViewSwitcher } from '../components/CalendarViewSwitcher';
import { CALENDAR_VIEW_ROUTE } from '../components/calendarViews';
import { resolveEventColor } from '../intl/eventColor';
import { useCacheReload } from '../state/cacheObserver';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Accessible day view — the screen-reader-first equivalent of the desktop
// calendar grid: a linear list of the selected day's events across all
// calendars, with previous/next-day navigation and create/edit/delete. Events
// read/write through the Host's on-device adapters (local + external).

/** Local-midnight → end-of-day UTC range for `date`, as RFC-3339 instants. */
function dayRangeUtc(date: Date): { start: string; end: string } {
  const start = new Date(date.getFullYear(), date.getMonth(), date.getDate(), 0, 0, 0, 0);
  const end = new Date(date.getFullYear(), date.getMonth(), date.getDate(), 23, 59, 59, 999);
  return { start: start.toISOString(), end: end.toISOString() };
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function EventsScreen({ navigation, route }: RootStackScreenProps<'Events'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  // The selected day at local midnight (date-only semantics for the heading).
  // Seeded from the Day⇄Agenda switcher's `anchor` param so switching keeps the
  // date, else today.
  const [day, setDay] = useState(() => {
    const seed = route.params?.anchor ? new Date(route.params.anchor) : new Date();
    const base = Number.isNaN(seed.getTime()) ? new Date() : seed;
    return new Date(base.getFullYear(), base.getMonth(), base.getDate());
  });
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Lookup tables for resolving each event's rendered colour (event label →
  // unmapped native colour → owning calendar's colour), rebuilt only when the
  // calendar set or palette changes.
  const calendarsById = useMemo(
    () => new Map(calendars.map((c) => [c.id, c])),
    [calendars],
  );
  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );

  const rowTags = useRef<Record<string, number | null>>({});

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const dayLabel = day.toLocaleDateString(i18n.language, {
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });

  const timeLabel = useCallback(
    (ev: CalendarEvent): string => {
      if (ev.all_day) return t('mobile.allDay');
      const fmt = (iso: string) =>
        new Date(iso).toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit' });
      return `${fmt(ev.start)}–${fmt(ev.end)}`;
    },
    [i18n.language, t],
  );

  // The event's rendered colour (a dot for sighted users) + the bound label's
  // name (appended to the accessible label so colour isn't the only signal —
  // only the event's OWN explicit label is named, matching the desktop).
  const eventLabel = useCallback(
    (ev: CalendarEvent): string => {
      const base = `${ev.title}, ${timeLabel(ev)}`;
      const { labelName } = resolveEventColor(ev, calendarsById, labelsById);
      return labelName
        ? `${base}${t('mobile.colorLabelSuffix', { name: labelName })}`
        : base;
    },
    [calendarsById, labelsById, t, timeLabel],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // listCalendars also primes the Host's route map, so it must run before
      // getEvents (which routes by calendar id). The palette is fetched in
      // parallel — it feeds the per-event colour dot (best-effort; a failure
      // just drops the colour cue, never the events).
      const [cals, labels] = await Promise.all([
        listCalendars(),
        listColorLabels().catch(() => [] as ColorLabel[]),
      ]);
      setCalendars(cals);
      setColorLabels(labels);
      const { start, end } = dayRangeUtc(day);
      const perCalendar = await Promise.all(
        cals.map((c) => getEvents({ calendar_id: c.id, start, end }).catch(() => [])),
      );
      // The backend returns the stored MASTER row for a recurring event, not
      // its per-day occurrences — so expand each series into the occurrences
      // that fall inside this day's range (rrule + EXDATE, shared with desktop)
      // and sort. Without this a recurring event is invisible on every day
      // after its first.
      const merged = expandAll(perCalendar.flat(), {
        start: new Date(start),
        end: new Date(end),
      });
      setEvents(merged);
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setLoading(false);
    }
  }, [announce, day, t]);

  // Reload whenever the day changes or the screen regains focus (after the
  // editor returns).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update while focused when an external calendar-cache refresh lands (the
  // root observer already announced it politely; this just re-reads the day).
  useCacheReload('calendar', load);

  const goToday = useCallback(() => {
    const now = new Date();
    setDay(new Date(now.getFullYear(), now.getMonth(), now.getDate()));
  }, []);


  const firstCalendarId = calendars[0]?.id ?? null;

  const addEvent = useCallback(() => {
    if (firstCalendarId == null) return;
    // Seed the new event on the day the user is viewing, not today.
    navigation.navigate('EventEditor', {
      eventId: null,
      calendarId: firstCalendarId,
      anchor: localDateKey(day),
    });
  }, [firstCalendarId, navigation, day]);

  // Editing an occurrence edits its underlying SERIES — the expanded row's id
  // is synthetic (`master@iso`), so resolve the real master id via seriesIdOf;
  // `occurrence` lets the editor offer the "this occurrence vs whole series" scope.
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

  // Events in a read-only calendar (synthetic birthday layers, iCal feeds) are
  // informational — no edit/delete (the adapter would reject a write anyway).
  const readOnlyIds = new Set(
    calendars.filter((c) => c.read_only).map((c) => c.id),
  );

  return (
    <View style={styles.screen}>
      <CalendarViewSwitcher
        active="day"
        // replace (not push): the calendar views are siblings, so swap in place
        // — keeps the stack flat (no duplicate back-stack entries) while the
        // fresh mount still picks up the anchor date from params. Pressing the
        // active view is suppressed by the switcher, so this only fires for
        // Week / Month / Agenda / Year.
        onSelect={(v) =>
          navigation.replace(CALENDAR_VIEW_ROUTE[v], { anchor: day.toISOString() })
        }
      />

      {/* Day navigation */}
      <View style={styles.dayBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.prevDay')}
          onPress={() => setDay((d) => addDays(d, -1))}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText}>‹</Text>
        </Pressable>
        <Text style={styles.dayHeading} accessibilityRole="header">
          {dayLabel}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.nextDay')}
          onPress={() => setDay((d) => addDays(d, 1))}
          style={({ pressed }) => [styles.navButton, pressed && styles.pressed]}
        >
          <Text style={styles.navButtonText}>›</Text>
        </Pressable>
      </View>

      {/* Jump straight to a date (YYYY-MM-DD) — submit on the field or the
          button; an unparseable date surfaces the inline error. */}
      <View style={styles.jumpBar}>
        <Text style={styles.jumpLabel} accessibilityRole="text">
          {t('mobile.jumpToDateNative')}
        </Text>
        {/* Native date picker (Apple-Reminders-style compact field) — commits on
            selection, so no separate "go" button + no manual YYYY-MM-DD parsing. */}
        <DateTimePicker
          mode="date"
          display="compact"
          value={day}
          onValueChange={(_, date) => {
            setError(null);
            setDay(new Date(date.getFullYear(), date.getMonth(), date.getDate()));
          }}
          locale={i18n.language}
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
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.calendarsButtonLabel')}
          onPress={() => navigation.navigate('Calendars')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.calendarsButtonLabel')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.search.title')}
          onPress={() => navigation.navigate('Search')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('toolbar.search')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: firstCalendarId == null }}
          accessibilityLabel={t('toolbar.newEvent')}
          disabled={firstCalendarId == null}
          onPress={addEvent}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.primaryPressed,
            firstCalendarId == null && styles.primaryDisabled,
          ]}
        >
          <Text style={styles.primaryButtonText}>{t('toolbar.newEvent')}</Text>
        </Pressable>
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('mobile.eventsLoading')}>
          {t('mobile.eventsLoading')}
        </Text>
      ) : events.length === 0 ? (
        <Text style={styles.muted}>{t('mobile.noEvents')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {events.map((ev) => {
            // The event's resolved colour: a small dot for sighted users (the
            // name rides the accessible label). Decorative → hidden from SR.
            const hex = resolveEventColor(ev, calendarsById, labelsById).hex;
            const dot =
              hex != null ? (
                <View
                  accessible={false}
                  importantForAccessibility="no"
                  style={[styles.colorDot, { backgroundColor: hex }]}
                />
              ) : null;
            return readOnlyIds.has(ev.calendar_id) ? (
              // Read-only calendar (a synthetic birthday layer or an iCal feed):
              // its events can't be edited or deleted, so the row is purely
              // informational — no edit/delete affordances, no "double-tap to
              // edit" hint that would mislead a screen-reader user.
              <View
                key={ev.id}
                accessible
                accessibilityRole="text"
                accessibilityLabel={eventLabel(ev)}
                style={styles.row}
              >
                {dot}
                <View style={styles.rowText}>
                  <Text style={styles.eventTitle}>{ev.title}</Text>
                  <Text style={styles.eventTime}>{timeLabel(ev)}</Text>
                </View>
              </View>
            ) : (
              <View
                key={ev.id}
                ref={(node) => {
                  rowTags.current[ev.id] = node ? findNodeHandle(node) : null;
                }}
                accessible
                accessibilityRole="button"
                accessibilityLabel={eventLabel(ev)}
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
                <Pressable
                  accessible={false}
                  onPress={() => editEvent(ev)}
                  style={styles.rowText}
                >
                  <Text style={styles.eventTitle}>{ev.title}</Text>
                  <Text style={styles.eventTime}>{timeLabel(ev)}</Text>
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
          })}
        </ScrollView>
      )}
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
    dayHeading: { flex: 1, fontSize: 18, fontWeight: '700', color: c.textPrimary, textAlign: 'center' },
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
    // Caption stacked ABOVE the native date chip (not beside it): the chip is a
    // native view whose intrinsic width RN can't reliably flex against, so a
    // side-by-side row let the label and the date overlap. Stacking is
    // overlap-proof and reads as a labelled control.
    jumpBar: {
      flexDirection: 'column',
      gap: 6,
      paddingHorizontal: 12,
      paddingTop: 12,
      alignItems: 'flex-start',
    },
    jumpLabel: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    actionBar: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, padding: 12, alignItems: 'center' },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    // Content-sized (NOT flex:1): with the action bar now wrapping, a flexed
    // primary button collapsed to a squished sliver once the row overflowed.
    // It still stands out by its accent fill.
    primaryButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    list: { gap: 12, padding: 16 },
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
    // A small colour dot for the event's resolved colour (sighted users); the
    // subtle border keeps light colours visible on the card. Matches TasksScreen.
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
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
