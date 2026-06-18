import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import {
  Calendar,
  CalendarEvent,
  deleteEvent as apiDeleteEvent,
  getEvents,
  listCalendars,
} from '../api/calendar';
import type { RootStackScreenProps } from '../navigation/types';

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

export default function EventsScreen({ navigation }: RootStackScreenProps<'Events'>) {
  const { t, i18n } = useTranslation();

  // The selected day at local midnight (date-only semantics for the heading).
  const [day, setDay] = useState(() => {
    const now = new Date();
    return new Date(now.getFullYear(), now.getMonth(), now.getDate());
  });
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

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

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // listCalendars also primes the Host's route map, so it must run before
      // getEvents (which routes by calendar id).
      const cals = await listCalendars();
      setCalendars(cals);
      const { start, end } = dayRangeUtc(day);
      const perCalendar = await Promise.all(
        cals.map((c) => getEvents({ calendar_id: c.id, start, end }).catch(() => [])),
      );
      const merged = perCalendar.flat().sort((a, b) => a.start.localeCompare(b.start));
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

  const goToday = useCallback(() => {
    const now = new Date();
    setDay(new Date(now.getFullYear(), now.getMonth(), now.getDate()));
  }, []);

  const firstCalendarId = calendars[0]?.id ?? null;

  const addEvent = useCallback(() => {
    if (firstCalendarId == null) return;
    navigation.navigate('EventEditor', { eventId: null, calendarId: firstCalendarId });
  }, [firstCalendarId, navigation]);

  const editEvent = useCallback(
    (ev: CalendarEvent) =>
      navigation.navigate('EventEditor', { eventId: ev.id, calendarId: ev.calendar_id }),
    [navigation],
  );

  const removeEvent = useCallback(
    (ev: CalendarEvent) => {
      Alert.alert(
        t('dialogs.confirm.deleteEventTitle'),
        t('dialogs.confirm.deleteEventMessage', { title: ev.title }),
        [
          { text: t('mobile.cancel'), style: 'cancel' },
          {
            text: t('dialogs.event.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                try {
                  await apiDeleteEvent(ev.id, ev.calendar_id, false);
                  announce(t('dialogs.event.deleted', { title: ev.title }));
                  await load();
                } catch (err) {
                  const message = errorMessage(err);
                  setError(message);
                  announce(t('mobile.error', { message }));
                }
              })();
            },
          },
        ],
      );
    },
    [announce, load, t],
  );

  // Events in a read-only calendar (synthetic birthday layers, iCal feeds) are
  // informational — no edit/delete (the adapter would reject a write anyway).
  const readOnlyIds = new Set(
    calendars.filter((c) => c.read_only).map((c) => c.id),
  );

  return (
    <View style={styles.screen}>
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
          {events.map((ev) =>
            readOnlyIds.has(ev.calendar_id) ? (
              // Read-only calendar (a synthetic birthday layer or an iCal feed):
              // its events can't be edited or deleted, so the row is purely
              // informational — no edit/delete affordances, no "double-tap to
              // edit" hint that would mislead a screen-reader user.
              <View
                key={ev.id}
                accessible
                accessibilityRole="text"
                accessibilityLabel={`${ev.title}, ${timeLabel(ev)}`}
                style={styles.row}
              >
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
                accessibilityLabel={`${ev.title}, ${timeLabel(ev)}`}
                accessibilityHint={t('mobile.taskHint')}
                accessibilityActions={[
                  { name: 'activate', label: t('mobile.editTaskLabel') },
                  { name: 'delete', label: t('dialogs.event.delete') },
                ]}
                onAccessibilityAction={(e) => {
                  if (e.nativeEvent.actionName === 'delete') removeEvent(ev);
                  else editEvent(ev);
                }}
                style={styles.row}
              >
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
            ),
          )}
        </ScrollView>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  dayBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 12,
    paddingTop: 14,
  },
  dayHeading: { flex: 1, fontSize: 18, fontWeight: '700', color: '#10131a', textAlign: 'center' },
  navButton: {
    width: 48,
    height: 48,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#f4f7fb',
  },
  navButtonText: { fontSize: 26, color: '#10131a', lineHeight: 30 },
  actionBar: { flexDirection: 'row', gap: 10, padding: 12, alignItems: 'center' },
  ghostButton: {
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  ghostButtonText: { fontSize: 16, fontWeight: '600', color: '#1d3a2f' },
  primaryButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  primaryPressed: { backgroundColor: '#1740a8' },
  primaryDisabled: { backgroundColor: '#9aa9c9' },
  primaryButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  list: { gap: 12, padding: 16 },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowText: { flex: 1, gap: 2 },
  eventTitle: { fontSize: 18, fontWeight: '600', color: '#10131a' },
  eventTime: { fontSize: 14, color: '#5b6573' },
  deleteButton: {
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#d9b3b0',
    backgroundColor: '#fbeceb',
  },
  deleteButtonText: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  pressed: { opacity: 0.7 },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318', paddingHorizontal: 16 },
});
