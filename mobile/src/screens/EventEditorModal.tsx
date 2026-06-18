import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { ColorLabel, Reminder } from '@aperio/shared';

import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { RadioGroup } from '../components/RadioGroup';
import { RecurrenceSelector } from '../components/RecurrenceSelector';
import { RemindersEditor } from '../components/RemindersEditor';
import {
  Calendar,
  CalendarEvent,
  createEvent,
  getEventById,
  listCalendars,
  updateEvent,
} from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
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

/** The start `YYYY-MM-DD` as a local Date for the recurrence selector's derived
 *  monthly/yearly options, or undefined when the field is empty/unparseable. */
function recurrenceStartDate(date: string): Date | undefined {
  if (!date.trim()) return undefined;
  const d = new Date(`${date.trim()}T00:00`);
  return Number.isNaN(d.getTime()) ? undefined : d;
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
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
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
  // The bound colour-label id ('' = none). Only LOCAL events carry it on their
  // own row; on an external calendar the colour is a host-local override (the
  // OverridesRepo path, deferred on mobile), so the picker is gated to local.
  const [colorLabel, setColorLabel] = useState('');
  // Reminders (relative-to-start / absolute / app-start), the same Reminder[]
  // the task editor edits — round-trips through create/update_event unchanged.
  const [reminders, setReminders] = useState<Reminder[]>([]);
  // The RRULE body (without "RRULE:"), or null = non-recurring. The series'
  // EXDATE exceptions are preserved from the original on save.
  const [recurrence, setRecurrence] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        // The palette feeds the colour picker (best-effort — a failure just
        // hides the picker's named options, never blocks the editor).
        const [cals, labels] = await Promise.all([
          listCalendars(),
          listColorLabels().catch(() => [] as ColorLabel[]),
        ]);
        setCalendars(cals);
        setColorLabels(labels);
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
            setColorLabel(ev.color_label ?? '');
            setReminders(ev.reminders ?? []);
            setRecurrence(ev.recurrence?.rrule ?? null);
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
    // Colour: a LOCAL event carries the chosen label on its own row; for an
    // external event the picker is hidden, so preserve whatever it loaded
    // (round-trip untouched) rather than letting an empty form value drop it.
    const isLocalCal = calendars.find((c) => c.id === calId)?.account_id === 'local';
    const colorToSend = isLocalCal ? colorLabel || null : (original?.color_label ?? null);
    // Keep the series' EXDATE exceptions when editing; a fresh rule has none.
    const recurrenceToSend = recurrence
      ? { rrule: recurrence, exceptions: original?.recurrence?.exceptions ?? [] }
      : null;
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
          color_label: colorToSend,
          reminders,
          recurrence: recurrenceToSend,
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
          recurrence: recurrenceToSend,
          color_label: colorToSend,
          reminders,
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
    calendars,
    colorLabel,
    description,
    editing,
    endDate,
    endTime,
    location,
    navigation,
    original,
    recurrence,
    reminders,
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

      {/* One switch node for SR (the Pressable carries role + checked + label and
          handles the tap); the inner Switch is the real visual toggle for
          sighted users, hidden from SR and non-interactive so the row stays a
          single accessible control. */}
      <Pressable
        accessibilityRole="switch"
        accessibilityState={{ checked: allDay }}
        accessibilityLabel={t('dialogs.event.fields.allDay')}
        onPress={() => setAllDay((v) => !v)}
        style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
      >
        <Text style={styles.switchLabel} importantForAccessibility="no">
          {t('dialogs.event.fields.allDay')}
        </Text>
        {/* The Switch is purely the visual indicator — the wrapping View's
            pointerEvents:'none' routes the tap to the Pressable (the one toggle
            owner), and it's hidden from SR so the row is a single switch node. */}
        <View style={styles.switchVisual}>
          <Switch
            value={allDay}
            trackColor={{ false: '#c9d2e0', true: '#1d4ed8' }}
            importantForAccessibility="no"
            accessibilityElementsHidden
          />
        </View>
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

      {/* Colour label — local calendars only (an external event's colour is a
          host-local override, deferred). Shows real swatches for sighted users
          + the name for SR; binds the event's own color_label. */}
      {calendars.find((c) => c.id === calId)?.account_id === 'local' && (
        <ColorLabelSelect
          value={colorLabel}
          labels={colorLabels}
          onChange={setColorLabel}
          disabled={saving}
        />
      )}

      {/* Recurrence — RRULE builder (freq / interval / weekly days / monthly
          mode / end), the same subset as the desktop event dialog. */}
      <RecurrenceSelector
        value={recurrence}
        onChange={setRecurrence}
        start={recurrenceStartDate(startDate)}
      />

      {/* Reminders — relative-to-start / absolute / app-start, the same editor
          as tasks (mode="event" labels the relative kind "Before start"). */}
      <RemindersEditor mode="event" value={reminders} onChange={setReminders} />

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
  switchVisual: { pointerEvents: 'none' },
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
