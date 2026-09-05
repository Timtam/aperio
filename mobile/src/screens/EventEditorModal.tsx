import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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

import type {
  ColorLabel,
  EventTimes,
  ExpandedOccurrence,
  Reminder,
} from '@aperio/shared';
import {
  allDayFormEndDate,
  applySignature,
  madeNoReminderChoice,
  signatureIn,
  eventPrefillFrom,
  allDayWireEnd,
  applyDateTimeChange,
  dateInput,
  defaultNewEventTimes,
  isBirthdayEventId,
  planSeriesSplit,
  recurrenceStartDate,
  selectableEventCalendars,
  writeSeriesSplit,
  timeInput,
  toIso,
} from '@aperio/shared';

import { AttendeesEditor } from '../components/AttendeesEditor';
import { TitleField } from '../components/TitleField';
import { TitleSuggestions } from '../components/TitleSuggestions';
import {
  rankEventSuggestions,
  useTitleSuggestions,
} from '../state/useTitleSuggestions';
import { AvailabilityChecker } from '../components/AvailabilityChecker';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { ConferenceSection } from '../components/ConferenceSection';
import { MeetingControls } from '../components/MeetingControls';
import { DateTimeFieldButton } from '../components/DateTimeFieldButton';
import { QuickTimeButton } from '../components/QuickTimeButton';
import { DescriptionLinks } from '../components/DescriptionLinks';
import { SignatureButton } from '../components/SignatureButton';
import {
  signatureForCalendar,
  useSignatures,
} from '../state/useSignatures';
import { EventRsvp } from '../components/EventRsvp';
import { FormScrollView } from '../components/FormScrollView';
import { SelectFieldButton } from '../components/SelectFieldButton';
import { RecurrenceSelector } from '../components/RecurrenceSelector';
import {
  ADAPTER_KIND_DEVICE_CALENDAR,
  listAccounts,
  type Account,
} from '../api/accounts';
import {
  listEventLocalReminders,
  setEventLocalReminders,
} from '../api/eventLocalReminders';
import {
  RemindersEditor,
  type EditableReminder,
} from '../components/RemindersEditor';
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
  planCarry,
  seriesIdOf,
  worthCarrying,
  type CarryableFields,
  type CarryScope,
} from '@aperio/shared';
import { eventGroupsForEvents } from '../api/eventGroups';
import type { RootStackScreenProps } from '../navigation/types';
import { useShowHiddenCalendarTargets } from '../settings/hiddenTargets';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import { writeLastUsedCalendar } from '../state/lastUsedCalendar';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import { useSoundPref } from '../state/useSoundPref';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Create / edit a calendar event. Screen-reader-first: every control is an
// addressable element with an explicit label; the calendar picker is a
// collapsed picker; all-day is a switch; start/end date + time are accessible
// DateTimeFieldButtons (always present — an event has a start and end). On edit the
// loaded event is sent back whole with the edits applied, so recurrence /
// reminders / attendees / the inline sound field round-trip untouched (the
// per-event sound OVERRIDE is a `sound.item.{id}` pref, edited below).

/** Split a stored RFC-3339 instant into the form's local `YYYY-MM-DD` +
 *  `HH:MM` strings. Built on the shared formatters (local components, never
 *  `toISOString`, which would shift the day east of GMT). */
function isoToLocalParts(iso: string): { date: string; time: string } {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return { date: '', time: '' };
  return { date: dateInput(d), time: timeInput(d) };
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The rows that ride ON the event — a real alarm the provider stores and
 *  every other client of the calendar sees. The placement flag is Aperio's own
 *  bookkeeping and never goes on the wire. */
function attachedRows(rows: readonly EditableReminder[]): Reminder[] {
  return rows
    .filter((r) => r.attach !== false)
    .map(({ kind, sound }) => ({ kind, sound }));
}

/** The rows Aperio keeps for this event alone (migration 0043). */
function privateRows(rows: readonly EditableReminder[]): Reminder[] {
  return rows
    .filter((r) => r.attach === false)
    .map(({ kind, sound }) => ({ kind, sound }));
}

export default function EventEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'EventEditor'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  const { hidden: hiddenCalendars } = useCalendarVisibility();
  const showHiddenCalendarTargets = useShowHiddenCalendarTargets();
  useCancelHeader(navigation);
  const {
    eventId,
    calendarId,
    occurrence,
    anchor,
    initialTitle,
    initialTime,
    initialScope,
    prefillFrom,
    targetPinned,
  } = route.params;
  const editing = eventId != null;
  // A single occurrence of a recurring series was opened (occurrence = its
  // instant) — offer the edit scope + seed the dates from the occurrence.
  const isOccurrence = occurrence != null;
  // Seeded from the up-front "this occurrence vs whole series" prompt
  // (eventEditScope). When the prompt set it, the control below is read-only.
  const [editScope, setEditScope] = useState<
    'occurrence' | 'series' | 'this_and_future'
  >(initialScope ?? 'occurrence');

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  // Accounts whose calendars never store a reminder Aperio writes: the device
  // calendar's alarms belong to the OS and the adapter drops what it is given.
  // "Attached" there would be a promise nothing keeps, so no choice is offered.
  const [deviceAccountIds, setDeviceAccountIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [original, setOriginal] = useState<CalendarEvent | null>(null);

  const [title, setTitle] = useState('');
  const [calId, setCalId] = useState(calendarId);
  // The four date/time fields + all-day live in ONE state object because they
  // are COUPLED: moving the start slides the end along (duration preserved),
  // and the end is clamped so it can never precede the start. That maths needs
  // all four values at once, which separate setters can't give atomically —
  // it's delegated to the shared, unit-tested `applyDateTimeChange` the desktop
  // EventDialog uses, so both platforms behave identically (Outlook/Google).
  const [times, setTimes] = useState<EventTimes>({
    startDate: '',
    startTime: '',
    endDate: '',
    endTime: '',
    allDay: false,
  });
  const { startDate, startTime, endDate, endTime, allDay } = times;
  const setDateTime = useCallback(
    (key: 'startDate' | 'startTime' | 'endDate' | 'endTime', value: string) => {
      setTimes((prev) => applyDateTimeChange(prev, key, value));
    },
    [],
  );
  const [location, setLocation] = useState('');
  /**
   * Earlier appointments with this name, offered while a NEW one is typed.
   *
   * Only while creating: opening an existing event and touching its title must
   * not offer to overwrite it with an older version of itself.
   */
  const titleMatches = useTitleSuggestions(title, 'events', !editing);
  const titleOptions = useMemo(
    () =>
      rankEventSuggestions(titleMatches, title).map(({ item }) => ({
        id: item.id,
        title: item.title,
        hint: calendars.find((c) => c.id === item.calendar_id)?.name,
      })),
    [titleMatches, title, calendars],
  );
  /**
   * Fill the editor from an earlier appointment.
   *
   * Split out from the suggestion list because the QUICK-ADD hands one in too:
   * picking an offer there opens this screen already filled, instead of
   * filling a one-line capture form the user then has to expand anyway.
   */
  const applyEventPrefill = useCallback(
    (source: CalendarEvent, opts: { keepCalendar?: boolean } = {}) => {
      const fill = eventPrefillFrom(source);
      setTitle(fill.title);
      setDescription(fill.description ?? '');
      setLocation(fill.location ?? '');
      setRecurrence(fill.rrule);
      setColorLabel(fill.color_label ?? '');
      setReminders((fill.reminders as Reminder[]).map((r) => ({ ...r, attach: true })));
      setAttendees(fill.attendees);
      // Attendees came along, so the invitation toggle goes OFF. Filling a
      // form from something written once is not the same as deciding to email
      // people about it, and that decision stays the user's.
      if (fill.attendees.length > 0) setNotifyAttendees(false);
      // The reminders are the user's own now, not the calendar's default.
      setKeepRemindersAsDefault(false);
      // The calendar the earlier appointment lived on — unless the caller
      // pinned one. Accepting an offer in this screen's own title field never
      // pins; the quick-add pins only when its picker was actually moved.
      if (
        !opts.keepCalendar &&
        calendars.some((c) => c.id === fill.calendar_id && !c.read_only)
      ) {
        setCalId(fill.calendar_id);
      }
      // The DAY stays exactly as it is — only the LENGTH travels, laid onto
      // whatever day the editor was opened on.
      setTimes((prev) => {
        if (fill.all_day) return { ...prev, allDay: true };
        const start = new Date(`${prev.startDate}T${prev.startTime || '00:00'}`);
        if (!Number.isFinite(start.getTime())) return prev;
        const end = new Date(start.getTime() + fill.durationMinutes * 60_000);
        return {
          ...prev,
          allDay: false,
          endDate: dateInput(end),
          endTime: timeInput(end),
        };
      });
    },
    [calendars],
  );

  const acceptTitleSuggestion = useCallback(
    (id: string) => {
      const source = titleMatches.find((e) => e.id === id);
      if (source) applyEventPrefill(source);
    },
    [titleMatches, applyEventPrefill],
  );

  // A prefill handed in by the quick-add. Once, after the form has its initial
  // state: the day and the calendar picked over there are already in it, and
  // the prefill deliberately leaves those alone.
  const prefillApplied = useRef(false);
  useEffect(() => {
    if (editing || !prefillFrom || prefillApplied.current || loading) return;
    prefillApplied.current = true;
    applyEventPrefill(prefillFrom, { keepCalendar: targetPinned === true });
  }, [editing, prefillFrom, loading, targetPinned, applyEventPrefill]);
  const [description, setDescription] = useState('');

  // The calendar's own signature, put on a NEW appointment by itself. A
  // binding is a default, not a button: a calendar that carries one should not
  // need a press per appointment. An existing event is never touched.
  //
  // On a calendar change the block is SWAPPED rather than stacked, and only
  // while it is still the one this editor put there — text the user wrote, or
  // a block they deleted, is theirs. Mail clients behave the same way when the
  // sending account changes.
  const { signatures } = useSignatures();
  const autoSignature = useRef<string | null>(null);
  useEffect(() => {
    if (editing || !calendarId) return;
    let cancelled = false;
    void signatureForCalendar(calendarId, signatures).then((sig) => {
      if (cancelled) return;
      const target = sig?.body?.trim() || null;
      setDescription((prev) => {
        const current = signatureIn(prev);
        const mine = autoSignature.current;
        if (target === null) {
          if (current === null || current !== mine) return prev;
          autoSignature.current = null;
          return applySignature(prev, '');
        }
        if (current === target) return prev;
        if (current !== null && current !== mine) return prev;
        autoSignature.current = target;
        return applySignature(prev, target);
      });
    });
    return () => {
      cancelled = true;
    };
  }, [editing, calendarId, signatures]);
  // The bound colour-label id ('' = none). Only LOCAL events carry it on their
  // own row; on an external calendar the colour is a host-local override (the
  // OverridesRepo path, deferred on mobile), so the picker is gated to local.
  const [colorLabel, setColorLabel] = useState('');
  // Reminders (relative-to-start / absolute / app-start), the same Reminder[]
  // the task editor edits — round-trips through create/update_event unchanged.
  // Each row says where it lives: attached rides ON the event (a real alarm
  // the provider stores and every other client of the calendar sees); the
  // others are reminders Aperio keeps for this appointment alone (migration
  // 0043). See `EditableReminder`.
  const [reminders, setReminders] = useState<EditableReminder[]>([]);
  // What was STORED for this event, and whether it actually reached the rows
  // on screen. An emptied list is a decision worth writing only if the stored
  // rows were there to be emptied; and the SIGNATURE is kept so a save that
  // does not describe the keyed event leaves it as it was.
  const privateSeedRef = useRef<{
    reminders: Reminder[];
    title: string;
    startsAt: string;
    landed: boolean;
  }>({ reminders: [], title: '', startsAt: '', landed: false });
  // True while the rows on screen came from the CALENDAR's default reminders
  // rather than from the event itself (see the overlay effect below). While it
  // holds, the save sends `[]` — the default stays a default instead of being
  // silently promoted to a per-event VALARM on the server. Mirrors the desktop
  // EventDialog's keepRemindersAsDefault.
  const [keepRemindersAsDefault, setKeepRemindersAsDefault] = useState(false);
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

  // Per-calendar default reminders (Settings → Kalender). Two jobs, one read:
  // while EDITING they re-overlay the opened event's calendar default, since
  // an entry applied at notification time is never in the event body and the
  // event would otherwise read as "no reminder" although one demonstrably
  // fires; while CREATING they are OFFERED for the calendar the appointment is
  // heading for, so an attached default can be seen, changed or removed before
  // it is written in.
  const calendarDefaults = useCalendarDefaultReminders(
    editing ? (original?.calendar_id ?? '') : calId,
  );
  // Guards the one-shot overlay: it must not fire twice for the same event and
  // must never clobber rows the user already edited (the pref read can resolve
  // a beat after the editor is usable).
  const overlaidForRef = useRef<string | null>(null);
  /** The rows the create offer above last put there, so a calendar change can
   *  swap them and anything the user changed is left alone. */
  const offeredRemindersRef = useRef<EditableReminder[]>([]);
  const remindersTouchedRef = useRef(false);
  // Which account the appointment is heading for, and whether saying "attached"
  // there would mean anything: a LOCAL calendar has no other client to tell,
  // and the DEVICE calendar's alarms are the OS's and are never written by
  // Aperio, so in both cases the choice would be one without a difference.
  const targetAccountId =
    calendars.find((c) => c.id === calId)?.account_id ?? 'local';
  const placementOffered =
    targetAccountId !== 'local' && !deviceAccountIds.has(targetAccountId);
  /**
   * The calendar's ATTACHED defaults, offered on a NEW appointment.
   *
   * The Host would attach them anyway (`use_calendar_defaults`), but silently:
   * the editor showed an empty list and the appointment came back carrying
   * alarms nobody could see, let alone remove or move. A default is an offer,
   * and an offer has to be on screen to be declined. Follows the calendar
   * picker while the rows are still untouched and still exactly what this
   * effect put there — anything the user changed is theirs.
   */
  useEffect(() => {
    if (editing || calendarDefaults.loading || remindersTouchedRef.current) return;
    const offered: EditableReminder[] = calendarDefaults.value
      .filter((d) => d.attach === true)
      .map(({ kind, sound }) => ({ kind, sound, attach: true }));
    setReminders((prev) => {
      const mine = offeredRemindersRef.current;
      // Rows the user (or a prefill from an earlier appointment) put there
      // stay: only an EMPTY list, or exactly what this effect last offered,
      // may be replaced by another calendar's offer. Empty has to count on its
      // own — the editor's own load resets the rows, and a guard that accepted
      // only `mine` would then see a list it no longer recognises and give up.
      // Untouched and empty is nothing to lose; a removal sets the touched
      // flag above and returns before ever reaching here.
      const adoptable =
        prev.length === 0 || JSON.stringify(prev) === JSON.stringify(mine);
      if (!adoptable) return prev;
      if (JSON.stringify(prev) === JSON.stringify(offered)) return prev;
      offeredRemindersRef.current = offered;
      return offered;
    });
  }, [editing, calId, calendarDefaults.loading, calendarDefaults.value]);

  useEffect(() => {
    if (!editing || original == null || calendarDefaults.loading) return;
    if (overlaidForRef.current === original.id) return;
    overlaidForRef.current = original.id;
    if (remindersTouchedRef.current) return;
    if ((original.reminders ?? []).length > 0) return;
    // Only an ATTACHED default belongs in these rows. The Host skips it as
    // soon as the event carries reminders of its own (`effective_reminders`),
    // so the row the user edits here really is the one that fires. An entry
    // that stays in Aperio fires ON TOP of the event's own reminders instead,
    // so showing it as an editable row would lie: changing 15 to 30 would not
    // move it, it would add a second reminder and leave the 15 ringing. That
    // entry belongs to the calendar, and the settings page says so.
    const attached = calendarDefaults.value.filter((d) => d.attach === true);
    if (attached.length === 0) return;
    // These ARE the calendar's ATTACHED defaults: the moment the user touches
    // them they ride on the appointment, so each row has to read as attached.
    // Whatever the private-reminder load already appended stays — that is
    // stored data, not a default this overlay owns.
    setReminders((prev) => [
      ...attached.map(({ kind, sound }) => ({ kind, sound, attach: true })),
      ...prev.filter((r) => r.attach === false),
    ]);
    setKeepRemindersAsDefault(true);
  }, [editing, original, calendarDefaults.loading, calendarDefaults.value]);

  useEffect(() => {
    void (async () => {
      try {
        // The palette feeds the colour picker (best-effort — a failure just
        // hides the picker's named options, never blocks the editor).
        const [cals, labels, accounts] = await Promise.all([
          listCalendars(),
          listColorLabels().catch(() => [] as ColorLabel[]),
          listAccounts().catch(() => [] as Account[]),
        ]);
        setCalendars(cals);
        setColorLabels(labels);
        setDeviceAccountIds(
          new Set(
            accounts
              .filter((a) => a.adapter_kind === ADAPTER_KIND_DEVICE_CALENDAR)
              .map((a) => a.id),
          ),
        );
        if (editing && eventId != null) {
          // Pass the route's calendarId so an EXTERNAL event resolves via the
          // SWR cache (the local store has no row for it) — otherwise the editor
          // opens empty + a save would duplicate it.
          const ev = await getEventById(eventId, calendarId);
          if (ev != null) {
            setOriginal(ev);
            setTitle(ev.title);
            setCalId(ev.calendar_id);
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
              setTimes({
                startDate: so.date,
                startTime: so.time,
                // All-day: map the (exclusive) end back to the last inclusive
                // day, else the editor shows one day too many (see the shared
                // allDayFormEndDate).
                endDate: ev.all_day ? allDayFormEndDate(occStart, occEnd) : eo.date,
                endTime: eo.time,
                allDay: ev.all_day,
              });
            } else {
              const s = isoToLocalParts(ev.start);
              const e = isoToLocalParts(ev.end);
              setTimes({
                startDate: s.date,
                startTime: s.time,
                endDate: ev.all_day
                  ? allDayFormEndDate(new Date(ev.start), new Date(ev.end))
                  : e.date,
                endTime: e.time,
                allDay: ev.all_day,
              });
            }
            setLocation(ev.location ?? '');
            setDescription(ev.description ?? '');
            setColorLabel(ev.color_label ?? '');
            setReminders((ev.reminders ?? []).map((r) => ({ ...r, attach: true })));
            // The private ones are not part of the event, so they arrive
            // separately and join the same list, each marked as what it is.
            void listEventLocalReminders()
              .then((rows) => {
                const seriesId = seriesIdOf(ev);
                const row = rows.find(
                  (r) => r.calendar_id === ev.calendar_id && r.event_id === seriesId,
                );
                // Never over an edit the user already made — and then these
                // rows are not on screen, so the save must not speak for the
                // stored ones either: their absence would be ignorance, not a
                // decision, and writing an empty list would destroy them.
                if (remindersTouchedRef.current) return;
                privateSeedRef.current = {
                  reminders: row?.reminders ?? [],
                  title: row?.title ?? '',
                  startsAt: row?.starts_at ?? '',
                  landed: true,
                };
                if (!row || row.reminders.length === 0) return;
                setReminders((prev) => [
                  ...prev,
                  ...row.reminders.map((r) => ({ ...r, attach: false })),
                ]);
              })
              .catch(() => {
                // Host unreachable: the event's own reminders still show, and
                // the next open tries again.
              });
            setRecurrence(ev.recurrence?.rrule ?? null);
            setAttendees(ev.attendees ?? []);
          }
        } else {
          // New event: a ONE-HOUR slot whose time-of-day adapts to context —
          // anchored on today → the next :00/:30 slot, on another day → 09:00,
          // with no anchor → the next full hour. Identical to the desktop
          // (shared defaultNewEventTimes); the old code seeded end = start,
          // so every new mobile event opened zero minutes long and refused to
          // save until the end was fixed by hand.
          let { start, end } = defaultNewEventTimes(anchor, new Date());
          // …unless the quick-add handed a picked start time over ("More
          // details …"); then honour it and keep the one-hour duration.
          if (initialTime && /^\d{2}:\d{2}$/.test(initialTime)) {
            const [h, m] = initialTime.split(':').map(Number);
            const picked = new Date(start);
            picked.setHours(h, m, 0, 0);
            start = picked;
            end = new Date(picked.getTime() + 60 * 60 * 1000);
          }
          setTimes({
            startDate: dateInput(start),
            startTime: timeInput(start),
            endDate: dateInput(end),
            endTime: timeInput(end),
            allDay: false,
          });
          // `initialTitle` carries the title typed into the event quick-add
          // before "More details …".
          if (initialTitle) setTitle(initialTitle);
        }
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
      } finally {
        setLoading(false);
      }
    })();
  }, [
    editing,
    eventId,
    calendarId,
    occurrence,
    anchor,
    initialTitle,
    initialTime,
    t,
  ]);

  /**
   * Offer to carry a saved edit to the group's other copies.
   *
   * Returns whether it navigated: silent (and `false`) when the event is in no
   * group, when nothing that travels changed, or when every other copy is
   * read-only — a screen that can only say "there is nothing to do" is worse
   * than no screen. When it does navigate it REPLACES the editor's own
   * goBack, so the user lands on one question instead of watching the editor
   * close and something else appear.
   */
  /**
   * The master as the EDITED OCCURRENCE looked, for the carry's "what changed".
   *
   * `original` is the series master — `editEventWithScope` navigates with
   * `seriesIdOf(ev)` and the editor loads that — so its start is the series'
   * DTSTART, weeks or months before the occurrence on screen. Handed to the
   * carry as the "before", every occurrence edit therefore looked like a move
   * of both start and end, and the copies had those instants written onto them
   * even when the user had only renamed the appointment.
   */
  const occurrenceBefore = useCallback(
    (master: CalendarEvent, occurrenceIso: string): CalendarEvent => {
      const duration = Math.max(
        0,
        new Date(master.end).getTime() - new Date(master.start).getTime(),
      );
      const startMs = new Date(occurrenceIso).getTime();
      if (!Number.isFinite(startMs)) return master;
      return {
        ...master,
        start: new Date(startMs).toISOString(),
        end: new Date(startMs + duration).toISOString(),
      };
    },
    [],
  );

  const offerToCarry = useCallback(
    async (
      saved: CalendarEvent,
      next: CalendarEvent,
      scope: CarryScope = 'series',
      occurrenceIso?: string | null,
    ): Promise<boolean> => {
      const fieldsOf = (ev: CalendarEvent): CarryableFields => ({
        title: ev.title,
        start: ev.start,
        end: ev.end,
        all_day: ev.all_day,
        location: ev.location,
        description: ev.description,
      });
      try {
        const groups = await eventGroupsForEvents([
          { calendar_id: saved.calendar_id, event_id: seriesIdOf(saved) },
        ]);
        const group = groups[0];
        if (!group) return false;
        const cals = await listCalendars();
        const anchor = {
          calendar_id: saved.calendar_id,
          event_id: seriesIdOf(saved),
        };
        const before = fieldsOf(saved);
        const after = fieldsOf(next);
        const plan = planCarry(
          group,
          anchor,
          before,
          after,
          (id) => {
            const cal = cals.find((c) => c.id === id);
            return cal != null && !cal.read_only;
          },
          (_cal, ev) => ev,
        );
        if (!worthCarrying(plan)) return false;
        // The carry screen lives on the CALENDAR stack only. The editor is
        // also reachable from Tasks and Settings (search results, a task's
        // linked event), and replacing with a route this navigator does not
        // have would strand the user in an editor that will not close. Ask
        // first; when it is not there, the save simply stands on its own.
        const reachable = navigation
          .getState()
          ?.routeNames?.includes('EventGroupCarry');
        if (!reachable) return false;
        navigation.replace('EventGroupCarry', {
          group,
          anchor,
          before,
          after,
          scope,
          occurrence: occurrenceIso,
          // An occurrence edit and a "this and all following" edit both leave
          // a NEW row behind, outside the group. The copies the carry makes
          // are outside it too, so they are tied to this one afterwards —
          // otherwise the appointment the user just made one row would be four
          // again from that point on.
          // Not for a row whose id carries `::rid::`: that is a provider-side
          // override, every group lookup resolves such a row through
          // `seriesIdOf` to its MASTER, and a member registered under the
          // composite id could never be matched by anything. The copies' new
          // rows are still tied to each other.
          successor:
            scope === 'series' || seriesIdOf(next) !== next.id
              ? null
              : {
                  calendar_id: next.calendar_id,
                  event_id: next.id,
                  title: next.title,
                  starts_at: next.start,
                },
        });
        return true;
      } catch {
        // Bookkeeping beside a save that already succeeded; failing it must
        // not report the save as failed.
        return false;
      }
    },
    [navigation],
  );


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
    const start = toIso(startDate, startTime, allDay);
    const end = allDay ? allDayWireEnd(endDate) : toIso(endDate, endTime, false);
    if (start == null || end == null) {
      setError(t('dialogs.event.dateInvalid'));
      return;
    }
    // Reject only an end BEFORE the start — a zero-length event is legal (and
    // the desktop accepts it). The strings are UTC ISO from the same builder,
    // but compare as instants so a differing sub-second/offset shape can't
    // flip the verdict.
    if (new Date(end).getTime() < new Date(start).getTime()) {
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
    // Reminders for the wire: while `keepRemindersAsDefault` holds, the rows on
    // screen came from the CALENDAR default and were never touched — sending
    // them would promote the default into a per-event VALARM that then lives on
    // independently of it. Send `[]` instead (same rule as the desktop).
    const remindersForWire = keepRemindersAsDefault ? [] : attachedRows(reminders);
    // The rows marked "only in Aperio" never go to the provider; they are
    // written to Aperio's own store after the save, against whatever id the
    // appointment ends up with. The signature travels with them so the row can
    // find its event again after the provider remints the id.
    // `keepRemindersAsDefault` is about the CALENDAR's defaults, which are
    // attached rows by construction. A private row was never a default — it
    // came out of the store — so the gate must not empty it, or an unrelated
    // save would delete the user's private reminder here and on every device.
    const privateReminders = privateRows(reminders);
    // An emptied list is still written when there WAS a row — that is the
    // record of the decision. A failure never fails the save: the appointment
    // is already stored, and a lost private reminder is worth less than an
    // error the user cannot act on.
    const savePrivate = async (saved: CalendarEvent) => {
      const seed = privateSeedRef.current;
      const hadSomethingToLose = seed.reminders.length > 0 && seed.landed;
      if (privateReminders.length === 0 && !hadSomethingToLose) return;
      const seriesId = seriesIdOf(saved);
      // The SIGNATURE has to describe the event the row is keyed by. A save
      // that carved one occurrence out of a series describes that occurrence,
      // not the series the row names — so only a save of the series itself
      // refreshes it; otherwise whatever was stored stands.
      const describesTheKeyedEvent = saved.id === seriesId;
      await setEventLocalReminders({
        calendar_id: saved.calendar_id,
        event_id: seriesId,
        reminders: privateReminders,
        title: describesTheKeyedEvent ? saved.title : seed.title || saved.title,
        starts_at: describesTheKeyedEvent ? saved.start : seed.startsAt || saved.start,
      }).catch(() => undefined);
      // The appointment moved to another calendar, or the provider minted a
      // new id: the old row names an event that is not there any more, and the
      // scan's repair could re-point it at whatever else shares its title and
      // start. Empty it — a peer holding the old one then stops firing.
      const oldEvent = original ? seriesIdOf(original) : null;
      const keyChanged =
        original != null &&
        oldEvent != null &&
        (original.calendar_id !== saved.calendar_id || oldEvent !== seriesId);
      if (keyChanged && seed.landed && seed.reminders.length > 0) {
        await setEventLocalReminders({
          calendar_id: original.calendar_id,
          event_id: oldEvent,
          reminders: [],
          title: seed.title,
          starts_at: seed.startsAt,
        }).catch(() => undefined);
      }
    };
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
          reminders: remindersForWire,
          sound: null,
          attendees,
          send_invitations: sendInvitations,
        });
        await savePrivate(created);
        if (!isLocalCal) {
          await setEventColor(created.id, calId, colorCapable ? null : colorToSend);
        }
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.event.occurrenceUpdated', { title: trimmedTitle }),
        );
        // The other copies have a series each, so carrying this means carving
        // the same occurrence out of them — not updating a row.
        if (
          await offerToCarry(
            occurrenceBefore(original, occurrence),
            created,
            'occurrence',
            occurrence,
          )
        ) {
          return;
        }
        navigation.goBack();
        return;
      }
      if (
        editing &&
        original != null &&
        isOccurrence &&
        occurrence != null &&
        editScope === 'this_and_future' &&
        original.recurrence?.rrule
      ) {
        // "This and all following": split the series at this occurrence. The
        // arithmetic — the COUNT the tail keeps, the EXDATEs that travel with
        // it, the zone it inherits — lives in `planSeriesSplit`; the order and
        // the recovery in `writeSeriesSplit`. See shared/seriesSplit.ts for why
        // each of those details decides whether the two halves line up.
        // The loaded `original` IS the master (getEventById resolves the
        // series), so its start anchors the occurrence count.
        const plan = planSeriesSplit(original, occurrence);
        if (plan == null) {
          throw new Error(t('dialogs.event.thisAndFutureLoadFailed', { title }));
        }
        const masterRecurrence = original.recurrence;
        const created = await writeSeriesSplit(
          {
            // Notify on the truncate too (symmetric with
            // delete-this-and-following): on notify-flag providers attendees
            // must learn the original series now ends before the cutoff, else
            // they keep the old occurrences AND receive the new tail invite.
            truncate: (headRule) =>
              updateEvent(
                {
                  ...original,
                  recurrence: { ...masterRecurrence, rrule: headRule },
                  send_invitations: sendInvitations,
                  truncate_tail_overrides: true,
                },
                original.calendar_id,
              ),
            createTail: (recurrence) =>
              createEvent(
                {
                  calendar_id: calId,
                  title: trimmedTitle,
                  description: description.trim() || null,
                  location: location.trim() || null,
                  start,
                  end,
                  all_day: allDay,
                  recurrence,
                  color_label: colorToSend,
                  reminders: remindersForWire,
                  sound: null,
                  attendees,
                  send_invitations: sendInvitations,
                },
                // Continuation of the master — keep its zone verbatim (incl.
                // floating) so head and tail expand identically.
                { preserveRecurrenceZone: true },
              ),
            restore: () =>
              updateEvent(
                { ...original, send_invitations: sendInvitations },
                original.calendar_id,
              ),
          },
          plan,
        );
        // The tail is a continuation of the same appointment, so the private
        // reminders follow it rather than staying on the head, which now ends
        // before the change the user just made.
        await savePrivate(created);
        if (!isLocalCal) {
          await setEventColor(created.id, calId, colorCapable ? null : colorToSend);
        }
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.event.thisAndFutureUpdated', { title: trimmedTitle }),
        );
        // The other copies have a series each, so carrying this means splitting
        // theirs at the same point — not updating a row.
        if (
          await offerToCarry(
            occurrenceBefore(original, occurrence),
            created,
            'future',
            occurrence,
          )
        ) {
          return;
        }
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
            reminders: remindersForWire,
            recurrence: recurrenceToSend,
            attendees,
            send_invitations: sendInvitations,
          },
          original.calendar_id,
        );
        await savePrivate(updated);
        // External calendar: a capable provider now stores the colour natively
        // (clear any stale override so the native value wins); a non-capable one
        // ignores it, so keep it as a host-local override. Local rides the row.
        if (!isLocalCal) {
          await setEventColor(updated.id, calId, colorCapable ? null : colorToSend);
        }
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.event.updated', { title: updated.title }),
        );
        // The appointment may exist several times over. Ask — after the save,
        // so the user's own change is never at stake — whether the other
        // copies should follow (DESIGN-event-groups.md, Stufe 2). Only for a
        // whole-event edit: the occurrence and this-and-following branches
        // return above, because "which copy of which occurrence" is a question
        // this cannot answer yet.
        if (await offerToCarry(original, updated)) return;
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
          reminders: remindersForWire,
          sound: null,
          attendees,
          send_invitations: sendInvitations,
          // An untouched editor made no reminder choice: the calendar's
          // "attach" mode may then write its defaults into the new event.
          use_calendar_defaults: madeNoReminderChoice(
            remindersTouchedRef.current,
            remindersForWire,
          ),
        });
        await savePrivate(created);
        if (!isLocalCal) {
          await setEventColor(created.id, calId, colorCapable ? null : colorToSend);
        }
        // Remember the calendar for the next new-event open. CREATES only:
        // an edit shouldn't bias future picks, since the user might have just
        // touched a series in a calendar they never write to.
        void writeLastUsedCalendar(calId);
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
    keepRemindersAsDefault,
    location,
    navigation,
    notifyAttendees,
    occurrence,
    occurrenceBefore,
    offerToCarry,
    original,
    recurrence,
    reminders,
    startDate,
    startTime,
    t,
    title,
  ]);

  // Delete with recurrence scope — the same shared confirm the list rows pop
  // (occurrence-vs-series for a recurring event, plain delete otherwise). The
  // loaded `original` is the series MASTER (getEventById), which carries no
  // occurrence context, so when the editor was opened FROM an occurrence row
  // re-attach the route's instant — gated on the freshly-loaded recurrence like
  // the edit-scope UI above (a stale occurrence param degrades to a plain
  // whole-event delete). On success announce + close, matching the row surfaces.
  const remove = useCallback(() => {
    if (original == null) return;
    const target: ExpandedOccurrence<CalendarEvent> | CalendarEvent =
      isOccurrence && occurrence != null && original.recurrence != null
        ? { ...original, series_id: original.id, occurrence_start: occurrence }
        : original;
    // The shared helper resolves organizer status and offers the cancel/silent
    // choice when this is a meeting we organize on a scheduling-capable provider.
    const cal = calendars.find((c) => c.id === original.calendar_id);
    confirmDeleteEvent(
      target,
      t,
      (message) => {
        AccessibilityInfo.announceForAccessibility(message);
        navigation.goBack();
      },
      (message) => {
        setError(message);
        AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
      },
      { supportsScheduling: cal?.supports_scheduling ?? false },
    );
  }, [calendars, isOccurrence, navigation, occurrence, original, t]);

  if (loading) {
    return (
      <View style={styles.screen}>
        <Text style={styles.muted} accessibilityLabel={t('mobile.loadingLabel')}>
          {t('mobile.loading')}
        </Text>
      </View>
    );
  }

  // Birthday events (DESIGN §10.3) are SYNTHESISED from contacts — there is no
  // row behind the id, so the load comes back empty and every field would be a
  // dead end whose save fails at the backend. Short-circuit to a read-only
  // summary the user can dismiss, exactly like the desktop EventDialog; the
  // contact is what they edit instead. The name comes from the row that opened
  // us (`initialTitle`) because the id can't be re-fetched.
  if (eventId != null && isBirthdayEventId(eventId)) {
    const name = original?.title ?? initialTitle ?? '';
    const age = original?.description ?? '';
    return (
      <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
        {name.length > 0 && <Text style={styles.birthdayName}>{name}</Text>}
        {age.length > 0 && (
          <Text style={styles.hint}>
            {t('dialogs.event.birthdayAge', { age })}
          </Text>
        )}
        <Text style={styles.hint}>{t('dialogs.event.birthdayHint')}</Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.event.birthdayClose')}
          onPress={() => navigation.goBack()}
          style={({ pressed }) => [styles.primaryButton, pressed && styles.primaryPressed]}
        >
          <Text style={styles.primaryButtonText}>
            {t('dialogs.event.birthdayClose')}
          </Text>
        </Pressable>
      </FormScrollView>
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
        <TitleField
          style={styles.input}
          value={title}
          onChangeText={setTitle}
          accessibilityLabel={t('dialogs.event.fields.title')}
        />
        <TitleSuggestions
          options={titleOptions}
          onAccept={acceptTitleSuggestion}
          editable={!saving}
        />
      </View>

      {calendars.length > 0 && (
        <SelectFieldButton<string>
          label={t('dialogs.event.fields.calendar')}
          value={calId}
          // Offer only calendars the user can write to AND hasn't hidden (the
          // Calendars-screen toggles), plus the event's own calendar so editing
          // one on a read-only / hidden source still shows it.
          options={selectableEventCalendars(calendars, {
            selectedIds: new Set(
              calendars
                .filter((c) => !hiddenCalendars.has(c.id))
                .map((c) => c.id),
            ),
            currentId: calId,
            includeHidden: showHiddenCalendarTargets,
          }).map((c) => ({ value: c.id, label: c.name }))}
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
        onPress={() => setTimes((p) => ({ ...p, allDay: !p.allDay }))}
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

      {/* Date/time FIELDS as accessible buttons (value in the label, picker in
          a dialog — DateTimeFieldButton): the inline compact picker never
          joined the VoiceOver swipe order. Each visible label is folded into
          its button's a11y label and hidden from the screen reader, so a field
          is one swipe stop, not two. */}
      <View style={styles.field}>
        <Text
          style={styles.label}
          accessibilityElementsHidden
          importantForAccessibility="no"
        >
          {t('dialogs.event.fields.startDate')}
        </Text>
        <DateTimeFieldButton
          label={t('dialogs.event.fields.startDate')}
          mode="date"
          value={startDate}
          onChange={(next) => setDateTime('startDate', next)}
        />
      </View>
      {!allDay && (
        <View style={styles.field}>
          <Text
            style={styles.label}
            accessibilityElementsHidden
            importantForAccessibility="no"
          >
            {t('dialogs.event.fields.startTime')}
          </Text>
          <DateTimeFieldButton
            label={t('dialogs.event.fields.startTime')}
            mode="time"
            value={startTime}
            onChange={(next) => setDateTime('startTime', next)}
          />
          <QuickTimeButton value={startTime} onPick={(next) => setDateTime('startTime', next)} />
        </View>
      )}

      <View style={styles.field}>
        <Text
          style={styles.label}
          accessibilityElementsHidden
          importantForAccessibility="no"
        >
          {t('dialogs.event.fields.endDate')}
        </Text>
        <DateTimeFieldButton
          label={t('dialogs.event.fields.endDate')}
          mode="date"
          value={endDate}
          onChange={(next) => setDateTime('endDate', next)}
        />
      </View>
      {!allDay && (
        <View style={styles.field}>
          <Text
            style={styles.label}
            accessibilityElementsHidden
            importantForAccessibility="no"
          >
            {t('dialogs.event.fields.endTime')}
          </Text>
          <DateTimeFieldButton
            label={t('dialogs.event.fields.endTime')}
            mode="time"
            value={endTime}
            onChange={(next) => setDateTime('endTime', next)}
          />
          <QuickTimeButton value={endTime} onPick={(next) => setDateTime('endTime', next)} />
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
        {/* Beside the field, not inside it: a signature is an addition at the
            end, and the text above stays the user's. */}
        <SignatureButton
          boundTo={calendarId}
          description={description}
          onChange={setDescription}
        />
        <DescriptionLinks text={description} />
      </View>

      {/* Any conference in this event, whoever created it — an Outlook or eM
          Client invitation as readily as one Aperio made. Detection is shared
          with the desktop and reads URLs rather than prose, so it does not
          depend on the invitation's language. */}
      <ConferenceSection location={location} description={description} />
      {/* Creating one, as opposed to joining one. Only for a meeting Aperio
          owns — an event carrying someone else's link gets Join above and no
          Remove here. */}
      <MeetingControls
        event={original}
        onEventChanged={(saved) => {
          setLocation(saved.location ?? '');
          setDescription(saved.description ?? '');
        }}
      />

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
          series" edits the master (incl. its recurrence rule). The scope is
          normally chosen in the up-front prompt (see eventEditScope), so the
          editor confirms it read-only — one clear choice beats a control a
          screen-reader user could miss. The segmented control stays as a fallback
          for any path that opens an occurrence without the prompt. */}
      {isOccurrence && original?.recurrence != null && initialScope != null && (
        <Text style={styles.muted}>
          {t('dialogs.event.scope.label')}:{' '}
          {t(
            editScope === 'occurrence'
              ? 'dialogs.event.scope.occurrence'
              : editScope === 'this_and_future'
                ? 'dialogs.event.scope.thisAndFuture'
                : 'dialogs.event.scope.series',
          )}
        </Text>
      )}
      {isOccurrence && original?.recurrence != null && initialScope == null && (
        <SelectFieldButton<'occurrence' | 'series' | 'this_and_future'>
          label={t('dialogs.event.scope.label')}
          value={editScope}
          options={[
            { value: 'occurrence', label: t('dialogs.event.scope.occurrence') },
            {
              value: 'this_and_future',
              label: t('dialogs.event.scope.thisAndFuture'),
            },
            { value: 'series', label: t('dialogs.event.scope.series') },
          ]}
          onChange={setEditScope}
        />
      )}

      {/* Editing a whole recurring series: say so, so a change to the times or
          the rule isn't mistaken for a one-off edit. Mirrors the desktop hint. */}
      {editing && original?.recurrence != null && !isOccurrence && (
        <Text style={styles.hint}>
          {t('dialogs.event.recurrence.editsSeries')}
        </Text>
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
      <RemindersEditor
        mode="event"
        value={reminders}
        // Where each reminder lives is a real choice on a calendar somebody
        // else can read: attached, the provider stores it and every client of
        // the calendar rings; only in Aperio, it stays here. A LOCAL calendar
        // has no such audience, so the choice would be one without a
        // difference and is not offered.
        placement={placementOffered}
        placementSurface="event"
        // The app-start collector reads an entry's own reminders from the
        // LOCAL store, and no wire format carries the kind — so anywhere else
        // it could never fire, attached or private. Same rule as the calendar
        // defaults: don't offer what stays silent.
        allowAppStart={targetAccountId === 'local'}
        onChange={(next) => {
          setReminders(next);
          // The moment the user touches the editor the rows become REAL
          // per-event reminders — drop the "keep as default" gate so the save
          // actually sends them, and block a late-resolving overlay.
          remindersTouchedRef.current = true;
          setKeepRemindersAsDefault(false);
        }}
      />

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
            start={toIso(startDate, startTime, allDay)}
            end={allDay ? allDayWireEnd(endDate) : toIso(endDate, endTime, false)}
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

      {/* Delete — edit-only (a new event has nothing to delete), after Save and
          visually destructive; disabled while a save runs. Hidden on a
          READ-ONLY calendar: the row surfaces gate their delete action the same
          way, but Search + Reminders open this editor for ANY event — without
          the gate the button would just dead-end in a provider rejection. */}
      {editing &&
        original != null &&
        !(calendars.find((c) => c.id === original.calendar_id)?.read_only ?? false) && (
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: saving }}
          accessibilityLabel={`${t('dialogs.event.delete')}: ${original.title}`}
          disabled={saving}
          onPress={remove}
          style={({ pressed }) => [
            styles.deleteButton,
            pressed && styles.pressed,
            saving && styles.deleteDisabled,
          ]}
        >
          <Text style={styles.deleteButtonText}>{t('dialogs.event.delete')}</Text>
        </Pressable>
      )}
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
    deleteButton: {
      marginTop: 8,
      paddingVertical: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
      alignItems: 'center',
    },
    deleteDisabled: { opacity: 0.5 },
    deleteButtonText: { fontSize: 16, fontWeight: '700', color: c.danger },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    birthdayName: { fontSize: 22, fontWeight: '700', color: c.textPrimary },
    hint: { fontSize: 15, color: c.textSecondary },
    pressed: { opacity: 0.7 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
