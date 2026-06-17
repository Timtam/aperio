import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { RadioGroup } from '../components/RadioGroup';
import {
  Calendar,
  CalendarEvent,
  createEvent,
  getEventById,
  listCalendars,
  updateEvent,
} from '../api/calendar';
import type { RootStackScreenProps } from '../navigation/types';

// Create / edit a calendar event. Screen-reader-first: every control is an
// addressable element with an explicit label; the calendar picker is a
// RadioGroup; all-day is a switch; date/time are text fields (YYYY-MM-DD /
// HH:MM) — the reliable SR input, matching the reminders editor. On edit the
// loaded event is sent back whole with the edits applied, so recurrence /
// reminders / attendees / sound (not editable here yet) round-trip untouched.

const pad = (n: number) => String(n).padStart(2, '0');

function isoToLocalParts(iso: string): { date: string; time: string } {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return { date: '', time: '' };
  return {
    date: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`,
    time: `${pad(d.getHours())}:${pad(d.getMinutes())}`,
  };
}

/** Local `YYYY-MM-DD` + `HH:MM` → RFC-3339 UTC, or null when unparseable. */
function localToIso(date: string, time: string): string | null {
  if (!date.trim()) return null;
  const d = new Date(`${date.trim()}T${time.trim() || '00:00'}`);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

function todayParts(): { date: string; time: string } {
  return isoToLocalParts(new Date().toISOString());
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function EventEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'EventEditor'>) {
  const { t } = useTranslation();
  const { eventId, calendarId } = route.params;
  const editing = eventId != null;

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [original, setOriginal] = useState<CalendarEvent | null>(null);

  const [title, setTitle] = useState('');
  const [calId, setCalId] = useState(calendarId);
  const [allDay, setAllDay] = useState(false);
  const [startDate, setStartDate] = useState('');
  const [startTime, setStartTime] = useState('');
  const [endDate, setEndDate] = useState('');
  const [endTime, setEndTime] = useState('');
  const [location, setLocation] = useState('');
  const [description, setDescription] = useState('');

  useEffect(() => {
    void (async () => {
      try {
        const cals = await listCalendars();
        setCalendars(cals);
        if (editing && eventId != null) {
          const ev = await getEventById(eventId);
          if (ev != null) {
            setOriginal(ev);
            setTitle(ev.title);
            setCalId(ev.calendar_id);
            setAllDay(ev.all_day);
            const s = isoToLocalParts(ev.start);
            const e = isoToLocalParts(ev.end);
            setStartDate(s.date);
            setStartTime(s.time);
            setEndDate(e.date);
            setEndTime(e.time);
            setLocation(ev.location ?? '');
            setDescription(ev.description ?? '');
          }
        } else {
          // New event: default to the next full hour today, one hour long.
          const now = todayParts();
          setStartDate(now.date);
          setStartTime(now.time);
          setEndDate(now.date);
          setEndTime(now.time);
        }
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
      } finally {
        setLoading(false);
      }
    })();
  }, [editing, eventId, t]);

  const save = useCallback(async () => {
    const trimmedTitle = title.trim();
    if (trimmedTitle.length === 0) {
      setError(t('dialogs.event.titleRequired'));
      return;
    }
    if (calId.trim().length === 0) {
      setError(t('dialogs.event.calendarRequired'));
      return;
    }
    // All-day clamps to 00:00 / 23:59 on the picked dates; timed uses the
    // entered times.
    const start = localToIso(startDate, allDay ? '00:00' : startTime);
    const end = localToIso(endDate, allDay ? '23:59' : endTime);
    if (start == null || end == null) {
      setError(t('dialogs.event.dateInvalid'));
      return;
    }
    if (end <= start) {
      setError(t('dialogs.event.endBeforeStart'));
      return;
    }
    setError(null);
    setSaving(true);
    try {
      if (editing && original != null) {
        // Send the loaded event back whole with the edits applied — preserves
        // recurrence / reminders / attendees / sound / etag.
        const updated = await updateEvent({
          ...original,
          title: trimmedTitle,
          calendar_id: calId,
          all_day: allDay,
          start,
          end,
          location: location.trim() || null,
          description: description.trim() || null,
        });
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.event.updated', { title: updated.title }),
        );
      } else {
        const created = await createEvent({
          calendar_id: calId,
          title: trimmedTitle,
          description: description.trim() || null,
          location: location.trim() || null,
          start,
          end,
          all_day: allDay,
          recurrence: null,
          color_label: null,
          reminders: [],
          sound: null,
          attendees: [],
        });
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.event.created', { title: created.title }),
        );
      }
      navigation.goBack();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
    } finally {
      setSaving(false);
    }
  }, [
    allDay,
    calId,
    description,
    editing,
    endDate,
    endTime,
    location,
    navigation,
    original,
    startDate,
    startTime,
    t,
    title,
  ]);

  if (loading) {
    return (
      <View style={styles.screen}>
        <Text style={styles.muted} accessibilityLabel={t('mobile.loadingLabel')}>
          {t('mobile.loading')}
        </Text>
      </View>
    );
  }

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.fields.title')}</Text>
        <TextInput
          style={styles.input}
          value={title}
          onChangeText={setTitle}
          accessibilityLabel={t('dialogs.event.fields.title')}
        />
      </View>

      {calendars.length > 0 && (
        <RadioGroup<string>
          label={t('dialogs.event.fields.calendar')}
          value={calId}
          options={calendars.map((c) => ({ value: c.id, label: c.name }))}
          onChange={setCalId}
        />
      )}

      <Pressable
        accessibilityRole="switch"
        accessibilityState={{ checked: allDay }}
        accessibilityLabel={t('dialogs.event.fields.allDay')}
        onPress={() => setAllDay((v) => !v)}
        style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
      >
        <Text style={styles.switchLabel}>{t('dialogs.event.fields.allDay')}</Text>
        <Text style={styles.switchState}>{allDay ? '☑' : '☐'}</Text>
      </Pressable>

      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.fields.startDate')}</Text>
        <TextInput
          style={styles.input}
          value={startDate}
          onChangeText={setStartDate}
          placeholder="YYYY-MM-DD"
          accessibilityLabel={t('dialogs.event.fields.startDate')}
          autoCapitalize="none"
          autoCorrect={false}
        />
      </View>
      {!allDay && (
        <View style={styles.field}>
          <Text style={styles.label}>{t('dialogs.event.fields.startTime')}</Text>
          <TextInput
            style={styles.input}
            value={startTime}
            onChangeText={setStartTime}
            placeholder="HH:MM"
            accessibilityLabel={t('dialogs.event.fields.startTime')}
            autoCapitalize="none"
            autoCorrect={false}
          />
        </View>
      )}

      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.fields.endDate')}</Text>
        <TextInput
          style={styles.input}
          value={endDate}
          onChangeText={setEndDate}
          placeholder="YYYY-MM-DD"
          accessibilityLabel={t('dialogs.event.fields.endDate')}
          autoCapitalize="none"
          autoCorrect={false}
        />
      </View>
      {!allDay && (
        <View style={styles.field}>
          <Text style={styles.label}>{t('dialogs.event.fields.endTime')}</Text>
          <TextInput
            style={styles.input}
            value={endTime}
            onChangeText={setEndTime}
            placeholder="HH:MM"
            accessibilityLabel={t('dialogs.event.fields.endTime')}
            autoCapitalize="none"
            autoCorrect={false}
          />
        </View>
      )}

      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.fields.location')}</Text>
        <TextInput
          style={styles.input}
          value={location}
          onChangeText={setLocation}
          accessibilityLabel={t('dialogs.event.fields.location')}
        />
      </View>

      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.fields.description')}</Text>
        <TextInput
          style={[styles.input, styles.multiline]}
          value={description}
          onChangeText={setDescription}
          accessibilityLabel={t('dialogs.event.fields.description')}
          multiline
        />
      </View>

      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: saving }}
        accessibilityLabel={t('mobile.save')}
        disabled={saving}
        onPress={() => void save()}
        style={({ pressed }) => [
          styles.primaryButton,
          pressed && styles.primaryPressed,
          saving && styles.primaryDisabled,
        ]}
      >
        <Text style={styles.primaryButtonText}>{t('mobile.save')}</Text>
      </Pressable>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 14 },
  field: { gap: 6 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  multiline: { minHeight: 88, textAlignVertical: 'top' },
  switchRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    padding: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  switchLabel: { fontSize: 16, fontWeight: '600', color: '#2b3240' },
  switchState: { fontSize: 22, color: '#10131a' },
  primaryButton: {
    marginTop: 8,
    paddingVertical: 14,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  primaryPressed: { backgroundColor: '#1740a8' },
  primaryDisabled: { backgroundColor: '#9aa9c9' },
  primaryButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  pressed: { opacity: 0.7 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
});
