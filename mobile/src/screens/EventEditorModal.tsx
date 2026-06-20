import { DateTimePicker } from '@expo/ui/community/datetime-picker';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { ColorLabel, Reminder } from '@aperio/shared';

import { AttendeesEditor } from '../components/AttendeesEditor';
import { AvailabilityChecker } from '../components/AvailabilityChecker';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { DescriptionLinks } from '../components/DescriptionLinks';
import { EventRsvp } from '../components/EventRsvp';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { RecurrenceSelector } from '../components/RecurrenceSelector';
import { RemindersEditor } from '../components/RemindersEditor';
import { SoundSelect } from '../components/SoundSelect';
import { useCancelHeader } from '../components/useCancelHeader';
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
import {
  formatLocalDate,
  formatLocalTime,
  parseLocalDate,
  parseLocalTime,
} from '../intl/dateTimeField';
import type { RootStackScreenProps } from '../navigation/types';
import { useSoundPref } from '../state/useSoundPref';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Create / edit a calendar event. Screen-reader-first: every control is an
// addressable element with an explicit label; the calendar picker is a
// RadioGroup; all-day is a switch; start/end date + time use the native
// @expo/ui DateTimePicker (always present — an event has a start and end). On edit the
// loaded event is sent back whole with the edits applied, so recurrence /
// reminders / attendees / the inline sound field round-trip untouched (the
// per-event sound OVERRIDE is a `sound.item.{id}` pref, edited below).

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

/** All-day end instant for the wire: the convention is END-EXCLUSIVE at the day
 *  level (the view's `daysCoveredKeys` walks `[start, endDay)` and breaks on the
 *  end day), so the form's last *inclusive* day `YYYY-MM-DD` stores as local
 *  midnight of the day AFTER it. Mirrors the desktop `allDayWireEnd`; without it
 *  a mobile-created multi-day all-day event drops its last day in every view. */
function allDayWireEnd(endDate: string): string | null {
  const d = endDate.trim();
  if (!d) return null;
  const date = new Date(`${d}T00:00`);
  if (Number.isNaN(date.getTime())) return null;
  date.setDate(date.getDate() + 1);
  return date.toISOString();
}

/** Inverse of {@link allDayWireEnd} for hydrating the form: the stored
 *  (exclusive) end instant maps back to the LAST covered day (end − 1 day, local
 *  time), clamped to the start's day so a legacy inclusive row (end == start)
 *  still hydrates to a valid single-day range. Mirrors the desktop
 *  `allDayFormEndDate`. */
function allDayFormEndDate(startIso: string, endIso: string): string {
  const start = new Date(startIso);
  const end = new Date(endIso);
  const lastDay = new Date(end.getFullYear(), end.getMonth(), end.getDate() - 1);
  const startDay = new Date(start.getFullYear(), start.getMonth(), start.getDate());
  const pick = lastDay.getTime() < startDay.getTime() ? startDay : lastDay;
  return `${pick.getFullYear()}-${pad(pick.getMonth() + 1)}-${pad(pick.getDate())}`;
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
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  useCancelHeader(navigation);
  const { eventId, calendarId, occurrence, anchor } = route.params;
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
  // Per-event sound OVERRIDE (§14.4 item level) — a host-local `sound.item.{id}`
  // pref, NOT the inline Event.sound (which the reminder resolver ignores). Keyed
  // by the loaded master id (so it's per-series). Edit-only: a new event has no
  // id yet, so it inherits the container/global default until re-edited (matches
  // the desktop, which hides this picker on create).
  const itemSound = useSoundPref(original ? `sound.item.${original.id}` : null);

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
          // Pass the route's calendarId so an EXTERNAL event resolves via the
          // SWR cache (the local store has no row for it) — otherwise the editor
          // opens empty + a save would duplicate it.
          const ev = await getEventById(eventId, calendarId);
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
              // All-day: map the (exclusive) end back to the last inclusive day,
              // else the editor shows one day too many (see allDayFormEndDate).
              setEndDate(
                ev.all_day
                  ? allDayFormEndDate(occStart.toISOString(), occEnd.toISOString())
                  : eo.date,
              );
              setEndTime(eo.time);
            } else {
              const s = isoToLocalParts(ev.start);
              const e = isoToLocalParts(ev.end);
              setStartDate(s.date);
              setStartTime(s.time);
              setEndDate(ev.all_day ? allDayFormEndDate(ev.start, ev.end) : e.date);
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
          // New event: the next full hour, one hour long, on the anchored day
          // (the tapped calendar day) when given, else today.
          const now = todayParts();
          const date = anchor ?? now.date;
          setStartDate(date);
          setStartTime(now.time);
          setEndDate(date);
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
  }, [editing, eventId, calendarId, occurrence, anchor, t]);

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
    // All-day: start = local midnight of the start day; end = local midnight of
    // the day AFTER the last picked day (END-EXCLUSIVE, the view convention —
    // see allDayWireEnd). Timed: the entered times.
    const start = localToIso(startDate, allDay ? '00:00' : startTime);
    const end = allDay ? allDayWireEnd(endDate) : localToIso(endDate, endTime);
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
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
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
        <DateTimePicker
          mode="date"
          display="compact"
          value={parseLocalDate(startDate)}
          onValueChange={(_, d) => setStartDate(formatLocalDate(d))}
          locale={i18n.language}
        />
      </View>
      {!allDay && (
        <View style={styles.field}>
          <Text style={styles.label}>{t('dialogs.event.fields.startTime')}</Text>
          <DateTimePicker
            mode="time"
            display="compact"
            value={parseLocalTime(startTime)}
            onValueChange={(_, d) => setStartTime(formatLocalTime(d))}
            locale={i18n.language}
          />
        </View>
      )}

      <View style={styles.field}>
        <Text style={styles.label}>{t('dialogs.event.fields.endDate')}</Text>
        <DateTimePicker
          mode="date"
          display="compact"
          value={parseLocalDate(endDate)}
          onValueChange={(_, d) => setEndDate(formatLocalDate(d))}
          locale={i18n.language}
        />
      </View>
      {!allDay && (
        <View style={styles.field}>
          <Text style={styles.label}>{t('dialogs.event.fields.endTime')}</Text>
          <DateTimePicker
            mode="time"
            display="compact"
            value={parseLocalTime(endTime)}
            onValueChange={(_, d) => setEndTime(formatLocalTime(d))}
            locale={i18n.language}
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
          capabilities={calendars.find((c) => c.id === calId)?.recurrence_capabilities}
        />
      )}

      {/* Reminders — relative-to-start / absolute / app-start, the same editor
          as tasks (mode="event" labels the relative kind "Before start"). */}
      <RemindersEditor mode="event" value={reminders} onChange={setReminders} />

      {/* Per-event sound override (§14.4 item level) — edit-only (a new event has
          no id to key the pref on yet; it inherits until re-edited). */}
      {editing && original != null && !itemSound.loading && (
        <SoundSelect
          label={t('reminders.sound.label')}
          value={itemSound.value}
          allowInherit
          onChange={(next) => void itemSound.save(next)}
          disabled={saving}
        />
      )}

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

      {/* Free/busy — attendee availability over the entered window. Shown only
          when there are attendees AND the target calendar advertises scheduling
          (a local calendar / non-scheduling provider returns no slots anyway).
          The window honours the all-day end-exclusive convention. */}
      {attendees.length > 0 &&
        (calendars.find((c) => c.id === calId)?.supports_scheduling ?? false) && (
          <AvailabilityChecker
            calendarId={calId}
            attendees={attendees}
            start={localToIso(startDate, allDay ? '00:00' : startTime)}
            end={allDay ? allDayWireEnd(endDate) : localToIso(endDate, endTime)}
          />
        )}

      {/* RSVP — only meaningful for an existing meeting that carries per-attendee
          response data (external, scheduling-capable providers); renders nothing
          otherwise. A successful response closes the editor so the list refetches
          the new status. */}
      {editing && original != null && (
        <EventRsvp event={original} onResponded={() => navigation.goBack()} />
      )}

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
    </FormScrollView>
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
