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

import { AttendeesEditor } from '../components/AttendeesEditor';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { DescriptionLinks } from '../components/DescriptionLinks';
import { RadioGroup } from '../components/RadioGroup';
import { RecurrenceSelector } from '../components/RecurrenceSelector';
import { RemindersEditor } from '../components/RemindersEditor';
import {
  addEventExdate,
  Calendar,
  CalendarEvent,
  createEvent,
  getEventById,
  listCalendars,
  updateEvent,
} from '../api/calendar';
import { listColorLabels } from '../api/colorLabels';
import { setEventColor } from '../api/containerColor';
import type { RootStackScreenProps } from '../navigation/types';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

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
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  const { eventId, calendarId, occurrence } = route.params;
  const editing = eventId != null;
  // A single occurrence of a recurring series was opened (occurrence = its
  // instant) — offer the edit scope + seed the dates from the occurrence.
  const isOccurrence = occurrence != null;
  const [editScope, setEditScope] = useState<'occurrence' | 'series'>('occurrence');

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
  // Free-form attendee strings ("Name <email>" / bare email) + whether to send
  // invitations (only meaningful on an external calendar with attendees).
  const [attendees, setAttendees] = useState<string[]>([]);
  const [notifyAttendees, setNotifyAttendees] = useState(true);

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
            // Editing a single occurrence shows ITS date (the master's start is
            // the first occurrence); keep the master's time-of-day + duration.
            // Gate on the freshly-loaded recurrence, not just the route param: if
            // the series lost its recurrence on another device between list-render
            // and edit, a stale occurrence param must degrade to a plain
            // whole-event edit (seed from the master), never move the master.
            if (occurrence != null && ev.recurrence != null) {
              const occStart = new Date(occurrence);
              const dur = new Date(ev.end).getTime() - new Date(ev.start).getTime();
              const occEnd = new Date(occStart.getTime() + (Number.isFinite(dur) ? dur : 0));
              const so = isoToLocalParts(occStart.toISOString());
              const eo = isoToLocalParts(occEnd.toISOString());
              setStartDate(so.date);
              setStartTime(so.time);
              setEndDate(eo.date);
              setEndTime(eo.time);
            } else {
              const s = isoToLocalParts(ev.start);
              const e = isoToLocalParts(ev.end);
              setStartDate(s.date);
              setStartTime(s.time);
              setEndDate(e.date);
              setEndTime(e.time);
            }
            setLocation(ev.location ?? '');
            setDescription(ev.description ?? '');
            setColorLabel(ev.color_label ?? '');
            setReminders(ev.reminders ?? []);
            setRecurrence(ev.recurrence?.rrule ?? null);
            setAttendees(ev.attendees ?? []);
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
  }, [editing, eventId, occurrence, t]);

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
    // Colour: the event carries the chosen label on the event itself — for a
    // LOCAL event it rides its row; for a colour-CAPABLE external calendar the
    // provider stores it as native COLOR; a non-capable provider ignores it,
    // and the colour lives as a host-local override (set after the save below).
    const cal = calendars.find((c) => c.id === calId);
    const isLocalCal = cal?.account_id === 'local';
    const colorCapable = isLocalCal || (cal?.supports_event_color ?? false);
    const colorToSend = colorLabel || null;
    // Invitations only go out when the target calendar advertises RFC-6638
    // scheduling (a local calendar, an iCal feed, or a CalDAV/iCloud account
    // whose scheduling probe failed all report supports_scheduling=false) AND
    // there are attendees AND the toggle is on. Mirrors the desktop's
    // supports_scheduling gating.
    const sendInvitations =
      (cal?.supports_scheduling ?? false) && attendees.length > 0 && notifyAttendees;
    // Keep the series' EXDATE exceptions when editing; a fresh rule has none.
    const recurrenceToSend = recurrence
      ? { rrule: recurrence, exceptions: original?.recurrence?.exceptions ?? [] }
      : null;
    setError(null);
    setSaving(true);
    try {
      if (
        editing &&
        original != null &&
        isOccurrence &&
        occurrence != null &&
        editScope === 'occurrence' &&
        original.recurrence != null
      ) {
        // "This occurrence only": exclude the original occurrence from the
        // series (add its instant to the master EXDATE), then create a STANDALONE
        // event (no recurrence) carrying the edits. Mirrors the desktop
        // EventDialog single-instance override.
        await addEventExdate(original.id, occurrence, original.calendar_id);
        const created = await createEvent({
          calendar_id: calId,
          title: trimmedTitle,
          description: description.trim() || null,
          location: location.trim() || null,
          start,
          end,
          all_day: allDay,
          recurrence: null,
          color_label: colorToSend,
          reminders,
          sound: null,
          attendees,
          send_invitations: sendInvitations,
        });
        if (!isLocalCal) {
          await setEventColor(created.id, calId, colorCapable ? null : colorToSend);
        }
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.event.occurrenceUpdated', { title: trimmedTitle }),
        );
        navigation.goBack();
        return;
      }
      if (editing && original != null) {
        // Send the loaded event back whole with the edits applied — preserves
        // recurrence / reminders / attendees / sound / etag. Pass the ORIGINAL
        // calendar so a calendar-picker change is detected as a cross-calendar
        // move (create-on-target + delete-from-source) rather than an in-place
        // PUT to a non-existent target resource (which an external provider
        // rejects 412). A cross-adapter move returns the new event at the target.
        const updated = await updateEvent(
          {
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
            attendees,
            send_invitations: sendInvitations,
          },
          original.calendar_id,
        );
        // External calendar: a capable provider now stores the colour natively
        // (clear any stale override so the native value wins); a non-capable one
        // ignores it, so keep it as a host-local override. Local rides the row.
        if (!isLocalCal) {
          await setEventColor(updated.id, calId, colorCapable ? null : colorToSend);
        }
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
          attendees,
          send_invitations: sendInvitations,
        });
        if (!isLocalCal) {
          await setEventColor(created.id, calId, colorCapable ? null : colorToSend);
        }
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
    attendees,
    calId,
    calendars,
    colorLabel,
    description,
    editScope,
    editing,
    endDate,
    endTime,
    isOccurrence,
    location,
    navigation,
    notifyAttendees,
    occurrence,
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
            trackColor={{ false: colors.border, true: colors.accent }}
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
        <DescriptionLinks text={description} />
      </View>

      {/* Colour — every calendar: a local or colour-capable external calendar
          stores it on the event (color_label); a non-capable external event
          keeps it as a host-local override (setEventColor on save). Real
          swatches for sighted users + the label name for SR. */}
      <ColorLabelSelect
        value={colorLabel}
        labels={colorLabels}
        onChange={setColorLabel}
        disabled={saving}
      />

      {/* Edit scope — only when a single occurrence of a recurring series was
          opened. "This occurrence only" excludes it + saves a standalone; "Whole
          series" edits the master (incl. its recurrence rule). */}
      {isOccurrence && original?.recurrence != null && (
        <RadioGroup<'occurrence' | 'series'>
          label={t('dialogs.event.scope.label')}
          value={editScope}
          options={[
            { value: 'occurrence', label: t('dialogs.event.scope.occurrence') },
            { value: 'series', label: t('dialogs.event.scope.series') },
          ]}
          onChange={setEditScope}
        />
      )}

      {/* Recurrence — RRULE builder (freq / interval / weekly days / monthly
          mode / end). Hidden when editing a single occurrence (the standalone
          it becomes is non-recurring); shown for new events + whole-series edits. */}
      {!(isOccurrence && editScope === 'occurrence') && (
        <RecurrenceSelector
          value={recurrence}
          onChange={setRecurrence}
          start={recurrenceStartDate(startDate)}
        />
      )}

      {/* Reminders — relative-to-start / absolute / app-start, the same editor
          as tasks (mode="event" labels the relative kind "Before start"). */}
      <RemindersEditor mode="event" value={reminders} onChange={setReminders} />

      {/* Attendees — free-form people; the notify switch shows only when the
          target calendar can actually invite (advertises RFC-6638 scheduling)
          and there are attendees, matching the desktop's gating. */}
      <AttendeesEditor
        value={attendees}
        onChange={setAttendees}
        notify={notifyAttendees}
        onNotifyChange={setNotifyAttendees}
        showNotify={
          attendees.length > 0 &&
          (calendars.find((c) => c.id === calId)?.supports_scheduling ?? false)
        }
      />

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

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 14 },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    multiline: { minHeight: 88, textAlignVertical: 'top' },
    switchRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    switchLabel: { fontSize: 16, fontWeight: '600', color: c.textLabel },
    switchVisual: { pointerEvents: 'none' },
    primaryButton: {
      marginTop: 8,
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    pressed: { opacity: 0.7 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
