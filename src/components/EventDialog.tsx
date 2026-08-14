import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import { DescriptionLinks } from './DescriptionLinks';
import {
  addEventExdate,
  createEvent as apiCreateEvent,
  deleteEventById,
  eventGroupsForEvents,
  getEventById,
  isCommandError,
  queryFreeBusy,
  setEventColor,
  updateEvent as apiUpdateEvent,
} from '../api/client';
import type { CalendarEvent, FreeBusy, FreeBusySlot } from '../api/types';
import {
  isExpandedOccurrence,
  isSeriesOccurrence,
  occurrenceIsoOf,
  planSeriesSplit,
  seriesIdOf,
  writeSeriesSplit,
} from '../intl/recurrence';
import {
  eventPrefillFrom,
  planCarry,
  worthCarrying,
  type CarryableFields,
  type CarryScope,
} from '@aperio/shared';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import { deleteThisAndFuture } from '../state/deleteSeriesFromOccurrence';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import { useCancellationChoice } from '../state/useCancellationChoice';
import { useViewState } from '../state/viewStateContext';
import { AttendeePicker } from './AttendeePicker';
import { ConfirmDialog } from './ConfirmDialog';
import { ColorLabelSelect } from './ColorLabelSelect';
import { ConferenceSection } from './ConferenceSection';
import { MeetingControls } from './MeetingControls';
import {
  allDayFormEndDate,
  allDayWireEnd,
  applyDateTimeChange,
  dateInput,
  defaultNewEventTimes,
  isBirthdayEventId,
  recurrenceStartDate,
  selectableEventCalendars,
  timeInput,
  toIso,
} from '@aperio/shared';
import { EventRsvp } from './EventRsvp';
import { TitleSuggestBox } from './TitleSuggestBox';
import {
  rankEventSuggestions,
  useTitleSuggestions,
} from '../state/useTitleSuggestions';
import { readLastUsedCalendar, writeLastUsedCalendar } from './lastUsedCalendar';
import { Modal } from './Modal';
import { RecurrenceSelector } from './RecurrenceSelector';
import { RemindersEditor } from './RemindersEditor';
import { SoundPrefField } from './SoundPrefField';
import type { Reminder } from '../api/types';

/**
 * Event create / edit dialog.
 *
 * Phase 4a covers the core fields from DESIGN.md section 7.2: title
 * (required), start/end (date + time), all-day toggle, location,
 * description, calendar. Recurrence, color labels, reminders, attendees,
 * and video-conference links land in subsequent waves.
 *
 * Pass `event=null` to create a new event; pass an existing event to
 * edit it. The dialog handles both with the same form.
 */
/**
 * Pull the bare email out of an attendee entry. The AttendeePicker
 * stores catalog picks as `"Name <email>"`; free-form entries are the
 * email itself. Free/busy needs just the address.
 */
function attendeeEmail(entry: string): string {
  const angle = entry.match(/<([^>]+)>/);
  return (angle ? angle[1] : entry).trim();
}

/**
 * True when any of `slots` overlaps the `[startIso, endIso)` window.
 * Compared via epoch millis so it's robust to time-zone/format
 * differences between the form's ISO and the backend's UTC slots.
 */
function isBusyInWindow(
  slots: FreeBusySlot[],
  startIso: string,
  endIso: string,
): boolean {
  const ws = new Date(startIso).getTime();
  const we = new Date(endIso).getTime();
  return slots.some(
    (s) => new Date(s.start).getTime() < we && new Date(s.end).getTime() > ws,
  );
}

export interface EventDialogProps {
  isOpen: boolean;
  onClose: () => void;
  event: CalendarEvent | null;
  /** Pre-selected calendar when creating a new event. */
  defaultCalendarId?: string;
  /**
   * ISO date (YYYY-MM-DD or full ISO) used to pre-fill start/end when
   * creating a new event. The dialog drops the time component so the
   * defaults are "next full hour on that day". Ignored when editing.
   */
  defaultDate?: string;
  /** Pre-fill the start time (HH:mm) when creating — carries the quick-add's
   *  picked time over the "weitere Details" hand-off, so the editor keeps it
   *  instead of re-deriving its own default slot. Ignored when editing. */
  defaultTime?: string;
  /** Pre-fill the title when creating — carries the in-progress title over
   *  from the event quick-add's "weitere Details" hand-off. Ignored when
   *  editing. */
  defaultTitle?: string;
  /** When editing a recurring occurrence, the scope the up-front prompt
   *  resolved to. Seeds the editScope radios; absent ⇒ 'occurrence'. */
  initialScope?: EditScope;
  /** An earlier appointment to fill this one from — the quick-add's hand-off
   *  when the user picked one of its offers. Everything but the day travels;
   *  see `eventPrefillFrom`. Create only. */
  prefillFrom?: CalendarEvent | null;
  /** The caller chose `defaultCalendarId` deliberately, so `prefillFrom` must
   *  leave it alone. The quick-add sets it only when its own picker was moved
   *  off the default. */
  targetPinned?: boolean;
}

interface FormState {
  title: string;
  calendarId: string;
  startDate: string;
  startTime: string;
  endDate: string;
  endTime: string;
  allDay: boolean;
  location: string;
  description: string;
  /** RRULE body (without "RRULE:" prefix), or null if non-recurring. */
  rrule: string | null;
  /** Color-label id, or null. */
  colorLabel: string | null;
  reminders: Reminder[];
  /** Free-form attendee strings (DESIGN.md §10.4). The
   *  AttendeePicker stores each pick from the contacts catalog
   *  as `"Name <email>"`; raw free-form entries (typed-in
   *  email addresses without a matching contact) round-trip
   *  verbatim. The adapter layer doesn't care about the format
   *  — it ships the array through to the source (CalDAV
   *  ATTENDEE, Graph attendees array, ...) as-is. */
  attendees: string[];
}

/**
 * Scope of edit/delete when the user opened a single occurrence of a
 * recurring series. `series` mutates the master row (and therefore
 * every future occurrence); `occurrence` adds the original date to
 * the series' EXDATE list and either creates an override standalone
 * event (edit) or simply skips that date (delete).
 */
type EditScope = 'series' | 'occurrence' | 'this_and_future';

/** True when the id carries the synthetic `@ISO` suffix from `expandEvent`. */
export function EventDialog({
  isOpen,
  onClose,
  event,
  defaultCalendarId,
  defaultDate,
  defaultTime,
  defaultTitle,
  initialScope,
  prefillFrom,
  targetPinned,
}: EventDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { openEventGroupCarry } = useDialogState();
  const { calendars, colorLabels, selectedCalendarIds } = useCalendarStore();
  const { showHiddenCalendarTargets } = useViewState();

  const isEdit = event !== null;
  // Stable id for the attendees-picker label — used as the
  // combobox's `aria-labelledby` so the input announces "Teilnehmer,
  // Combobox" with the right name on every screen reader.
  const attendeesLabelId = useId();

  // Per-calendar default reminders (Settings → Kalender). The point of
  // these is to mirror iOS's "Standard-Hinweise", which iOS applies
  // locally instead of writing into the VEVENT body — so iCloud sends
  // us events with `reminders: []` and we re-overlay the user's chosen
  // calendar default at form-init time. The overlay is intentionally
  // confined to this dialog (not the wire layer) so opening + saving
  // an iCloud event without touching anything doesn't silently promote
  // the calendar default into a per-event VALARM on the server.
  const dialogCalendarId = event?.calendar_id;
  const calendarIdsForDefaults = useMemo(
    () => (dialogCalendarId ? [dialogCalendarId] : []),
    [dialogCalendarId],
  );
  const { getDefaultsFor } = useCalendarDefaultReminders(
    calendarIdsForDefaults,
  );

  // True when the form's `reminders` slot was filled from the
  // calendar's default rather than from the event itself. Used by the
  // submit path to send `[]` instead of the defaults — keeps the wire
  // pure unless the user explicitly touches the reminders editor.
  const remindersWereFromDefault =
    isEdit &&
    event !== null &&
    (event.reminders ?? []).length === 0 &&
    getDefaultsFor(event.calendar_id).length > 0;

  const initialState = useMemo<FormState>(() => {
    const base = buildInitialState(
      event,
      defaultCalendarId,
      defaultDate,
      defaultTime,
      defaultTitle,
      calendars,
      selectedCalendarIds,
    );
    if (remindersWereFromDefault && event) {
      return {
        ...base,
        reminders: getDefaultsFor(event.calendar_id),
      };
    }
    return base;
  }, [
    event,
    defaultCalendarId,
    defaultDate,
    defaultTime,
    defaultTitle,
    calendars,
    selectedCalendarIds,
    remindersWereFromDefault,
    getDefaultsFor,
  ]);

  /**
   * Offer to carry a saved edit to the group's other copies.
   *
   * Silent when the event is in no group, when nothing that travels changed,
   * or when every other copy is read-only — a dialog that can only say "there
   * is nothing to do" is worse than no dialog. The editor closes first: the
   * question is about what happens NEXT, and stacking it over the editor it
   * came from is how focus ends up somewhere nobody asked for.
   */
  const offerToCarry = useCallback(
    async (
      saved: CalendarEvent,
      next: CalendarEvent,
      scope: CarryScope = 'series',
      occurrence?: string | null,
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
            const cal = calendars.find((c) => c.id === id);
            return cal != null && !cal.read_only;
          },
          (_cal, ev) => ev,
        );
        if (!worthCarrying(plan)) return false;
        // An occurrence edit and a "this and all following" edit both leave a
        // NEW row behind, outside the group. The copies the carry makes are
        // outside it too, so they are tied to this one afterwards — otherwise
        // the appointment the user just made one row would be four again from
        // that point on.
        // Not for a row whose id carries `::rid::`: that is a provider-side
        // override, every group lookup resolves such a row through
        // `seriesIdOf` to its MASTER, and a member registered under the
        // composite id could never be matched by anything. The copies' new
        // rows are still tied to each other.
        const successor =
          scope === 'series' || seriesIdOf(next) !== next.id
            ? null
            : {
                calendar_id: next.calendar_id,
                event_id: next.id,
                title: next.title,
                starts_at: next.start,
              };
        // CLOSE FIRST, then ask. The editor's own `onClose` pops the top frame,
        // and pushing before it meant popping the question instead of the
        // editor: the dialog never appeared and the editor stayed open — stage
        // 2 did nothing at all on this platform.
        onClose();
        openEventGroupCarry({
          group,
          anchor,
          before,
          after,
          scope,
          occurrence,
          successor,
        });
        return true;
      } catch {
        // The grouping lookup is bookkeeping beside a save that already
        // succeeded; failing it must not report the save as failed.
        return false;
      }
    },
    [calendars, onClose, openEventGroupCarry],
  );


  const [form, setForm] = useState<FormState>(initialState);
  /**
   * Earlier appointments with this name, offered while a NEW one is typed.
   *
   * Only while creating: opening an existing event and touching its title must
   * not offer to overwrite it with an older version of itself.
   */
  const titleMatches = useTitleSuggestions(form.title, 'events', !isEdit && isOpen);
  const titleOptions = useMemo(
    () =>
      rankEventSuggestions(titleMatches, form.title).map(({ item }) => ({
        id: item.id,
        title: item.title,
        hint: calendars.find((c) => c.id === item.calendar_id)?.name,
      })),
    [titleMatches, form.title, calendars],
  );
  /**
   * Fill the editor from an earlier appointment.
   *
   * Split out from the suggestion list because the QUICK-ADD hands one in
   * too: picking an offer there opens this editor already filled, instead of
   * filling a one-line capture form the user then has to expand anyway.
   */
  const applyEventPrefill = useCallback(
    (source: CalendarEvent, opts: { keepCalendar?: boolean } = {}) => {
      const fill = eventPrefillFrom(source);
      setForm((prev) => {
        // The DAY stays exactly as it was — it is what makes this a new
        // appointment, and it came from wherever the user opened the editor.
        // Only the LENGTH travels, laid onto that day.
        const start = new Date(`${prev.startDate}T${prev.startTime || '00:00'}`);
        const end = new Date(start.getTime() + fill.durationMinutes * 60_000);
        return {
          ...prev,
          title: fill.title,
          description: fill.description ?? '',
          location: fill.location ?? '',
          allDay: fill.all_day,
          endDate: fill.all_day ? prev.endDate : dateInput(end),
          endTime: fill.all_day ? prev.endTime : timeInput(end),
          rrule: fill.rrule,
          colorLabel: fill.color_label,
          reminders: fill.reminders as Reminder[],
          attendees: fill.attendees,
          // The calendar the earlier appointment lived on — unless the caller
          // pinned one. Accepting an offer in this editor's own title field
          // never pins, so there the old calendar travels; the quick-add pins
          // only when the user actually picked something there instead of
          // leaving its default.
          calendarId:
            !opts.keepCalendar &&
            calendars.some((c) => c.id === fill.calendar_id && !c.read_only)
              ? fill.calendar_id
              : prev.calendarId,
        };
      });
      // Attendees came along, so the invitation toggle goes OFF. Filling a
      // form from something you wrote once is not the same as deciding to
      // email eight people about it, and that decision has to stay the user's
      // — the toggle is right there, and it is announced.
      if (fill.attendees.length > 0) setNotifyAttendees(false);
      // The reminders came from a real earlier event, so they are the user's
      // own and must be written as such — not treated as the calendar default
      // the editor would otherwise send as an empty list.
      setKeepRemindersAsDefault(false);
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

  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  // Flips to `false` the moment the user touches the reminders editor.
  // While still `true` on submit, the dialog sends an empty reminders
  // list — the calendar default stays a default and isn't silently
  // promoted to a per-event VALARM the user never asked for.
  const [keepRemindersAsDefault, setKeepRemindersAsDefault] = useState(
    remindersWereFromDefault,
  );
  // "Notify attendees": defaults ON. Only meaningful (and only shown) when
  // the calendar supports server-side scheduling AND the event has
  // attendees; on submit we gate the wire flag on those too.
  const [notifyAttendees, setNotifyAttendees] = useState(true);

  // Attendee free/busy: result of the last "Check availability" run,
  // plus the window it was queried for (so the per-attendee busy verdict
  // is computed against the exact slot the user asked about). `null`
  // until the user runs a check; cleared whenever the attendees or the
  // start/end window change so we never show a stale verdict.
  const [availability, setAvailability] = useState<FreeBusy[] | null>(null);
  const [availabilityWindow, setAvailabilityWindow] = useState<{
    start: string;
    end: string;
  } | null>(null);
  const [checkingAvailability, setCheckingAvailability] = useState(false);
  const [availabilityError, setAvailabilityError] = useState<string | null>(
    null,
  );

  // When editing a single occurrence of a recurring series the user
  // can apply changes to just this occurrence (creates an EXDATE +
  // standalone override) or to the whole series.
  // Expanded from a master OR a provider-sent override — both are one
  // occurrence of a series, and both have to offer the scope choice.
  const isOccurrence = isEdit && !!event && isSeriesOccurrence(event);
  const [editScope, setEditScope] = useState<EditScope>(
    initialScope ?? 'occurrence',
  );

  // Live mirror of the form for the pristine check below — a ref, so the reset
  // effect reads the CURRENT form without listing it as a dep (which would
  // re-run it on every keystroke).
  const formRef = useRef(form);
  formRef.current = form;
  // The initialState this dialog last reset to — the pristine baseline.
  const appliedInitialRef = useRef<FormState | null>(null);

  // Reset the form whenever the dialog is opened with new context.
  //
  // `initialState` is a useMemo over `calendars` + `selectedCalendarIds`, so it
  // re-derives identity on every background calendar-catalog refresh
  // (CacheSyncListener → refreshCalendars) while the dialog is open. Resetting
  // then unmounted the focused attendee/notify controls (focus fell to <body>,
  // NVDA switched to browse mode with the dialog still up) AND silently wiped
  // everything the user had typed. So: adopt a fresh initialState only while the
  // form is still PRISTINE (byte-identical to the last one we applied). Once the
  // user has touched anything, their edits win until the dialog closes.
  //
  // `initialState` is not final at first render: the calendar default-reminders
  // overlay hydrates via an async pref read, and a cold calendar store resolves
  // the default calendarId a beat late — both re-derive `initialState` shortly
  // after mount. So `setForm` (and its COUPLED keep-as-default flag — applying
  // the overlaid reminders while leaving the flag false would submit them as
  // per-event VALARMs) must adopt the fresh initialState on EVERY pristine run,
  // exactly like ContactDialog. Only the toggles that are INDEPENDENT of `form`
  // (edit scope, notify) and the transient availability/error resets are gated
  // to the first hydrate — re-arming those on an incidental pristine churn would
  // undo an untouched-form change like the user unchecking "notify attendees".
  useEffect(() => {
    if (!isOpen) {
      appliedInitialRef.current = null;
      return;
    }
    const baseline = appliedInitialRef.current;
    const firstHydrate = baseline === null;
    const pristine =
      firstHydrate ||
      formRef.current === baseline ||
      JSON.stringify(formRef.current) === JSON.stringify(baseline);
    if (!pristine) return; // user touched the form → their edits win
    appliedInitialRef.current = initialState;
    setForm(initialState);
    setKeepRemindersAsDefault(remindersWereFromDefault);
    if (!firstHydrate) return; // form tracked; leave the independent toggles alone
    setError(null);
    setEditScope(initialScope ?? 'occurrence');
    setNotifyAttendees(true);
    setAvailability(null);
    setAvailabilityWindow(null);
    setAvailabilityError(null);
  }, [isOpen, initialState, remindersWereFromDefault, initialScope]);

  /**
   * A prefill handed in by the quick-add, applied once per opening.
   *
   * Declared AFTER the reset effect on purpose. Effects run in declaration
   * order, and the reset's first run takes the `firstHydrate` shortcut — it
   * treats the form as pristine without comparing anything, because there is
   * no baseline yet. Applying the prefill above it meant the reset overwrote
   * everything a moment later, in the same commit: only the title survived,
   * because that one rides `defaultTitle` into `initialState` instead. From
   * here the reset has already run, so the prefill lands on top — and every
   * LATER pristine re-run (a background calendar-catalog refresh re-deriving
   * `initialState`) now sees a form that differs from the baseline and leaves
   * it alone.
   *
   * `targetPinned` is the quick-add saying it does not want its calendar
   * touched, because the user chose one there rather than leaving the default.
   */
  const prefillApplied = useRef<string | null>(null);
  useEffect(() => {
    if (!isOpen || isEdit || !prefillFrom) {
      if (!isOpen) prefillApplied.current = null;
      return;
    }
    if (prefillApplied.current === prefillFrom.id) return;
    prefillApplied.current = prefillFrom.id;
    applyEventPrefill(prefillFrom, { keepCalendar: targetPinned === true });
  }, [isOpen, isEdit, prefillFrom, targetPinned, applyEventPrefill]);

  // Any change to the attendee set, the start/end window, the all-day
  // flag, or the target calendar invalidates a previous availability
  // check — drop it so we never render a busy/free verdict against a
  // window the user has since moved.
  useEffect(() => {
    setAvailability(null);
    setAvailabilityWindow(null);
    setAvailabilityError(null);
  }, [
    form.attendees,
    form.startDate,
    form.startTime,
    form.endDate,
    form.endTime,
    form.allDay,
    form.calendarId,
  ]);

  const update = useCallback(
    <K extends keyof FormState>(key: K, value: FormState[K]) => {
      setForm((prev) => {
        // The four date/time fields are coupled: editing the start
        // slides the end along (preserving duration), editing the end
        // is clamped so it can't precede the start. Outlook/Google
        // behaviour, delegated to a pure, unit-tested helper.
        if (
          key === 'startDate' ||
          key === 'startTime' ||
          key === 'endDate' ||
          key === 'endTime'
        ) {
          return {
            ...prev,
            ...applyDateTimeChange(prev, key, value as string),
          };
        }
        return { ...prev, [key]: value };
      });
    },
    [],
  );

  // "Check availability": query the attendees' free/busy over the
  // currently-entered window and surface who's busy. Best-effort — a
  // provider that can't answer (no scheduling, permission denied) comes
  // back with empty slots, which simply read as "free/unknown" rather
  // than failing the dialog.
  const checkAvailability = useCallback(async () => {
    const start = toIso(form.startDate, form.startTime, form.allDay);
    // All-day wire end is exclusive (last day + 1) so the free/busy
    // window actually covers the event's final day.
    const end = form.allDay
      ? allDayWireEnd(form.endDate)
      : toIso(form.endDate, form.endTime, false);
    if (!start || !end) {
      setAvailabilityError(t('dialogs.event.dateInvalid'));
      return;
    }
    const emails = form.attendees.map(attendeeEmail).filter(Boolean);
    if (emails.length === 0) return;

    setCheckingAvailability(true);
    setAvailabilityError(null);
    try {
      const result = await queryFreeBusy(form.calendarId, emails, start, end);
      setAvailability(result);
      setAvailabilityWindow({ start, end });
      const busyCount = result.filter((fb) =>
        isBusyInWindow(fb.slots, start, end),
      ).length;
      announce(
        busyCount === 0
          ? t('dialogs.event.availability.allFree')
          : t('dialogs.event.availability.someBusy', { count: busyCount }),
      );
    } catch (err) {
      setAvailabilityError(
        isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
      );
    } finally {
      setCheckingAvailability(false);
    }
  }, [
    form.attendees,
    form.startDate,
    form.startTime,
    form.endDate,
    form.endTime,
    form.allDay,
    form.calendarId,
    announce,
    t,
  ]);

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      if (submitting) return; // re-entry guard while a slow PUT is in flight
      setError(null);

      const trimmedTitle = form.title.trim();
      if (!trimmedTitle) {
        setError(t('dialogs.event.titleRequired'));
        return;
      }
      if (!form.calendarId) {
        setError(t('dialogs.event.calendarRequired'));
        return;
      }

      const start = toIso(form.startDate, form.startTime, form.allDay);
      // All-day events go on the wire with an EXCLUSIVE end (last day + 1,
      // local midnight) — the convention the views and every provider
      // adapter assume. The form's end-date input stays inclusive.
      const end = form.allDay
        ? allDayWireEnd(form.endDate)
        : toIso(form.endDate, form.endTime, false);
      if (!start || !end) {
        setError(t('dialogs.event.dateInvalid'));
        return;
      }
      if (new Date(end).getTime() < new Date(start).getTime()) {
        setError(t('dialogs.event.endBeforeStart'));
        return;
      }

      const recurrence = form.rrule
        ? { rrule: form.rrule, exceptions: event?.recurrence?.exceptions ?? [] }
        : null;

      setSubmitting(true);
      try {
        // The reminders list to send to the server. When
        // `keepRemindersAsDefault` is still true, the editor was never
        // touched and the rows the user is looking at came from the
        // calendar's default — sending them would silently promote
        // the default to a per-event VALARM that lives on the wire
        // independently of the default. Sending `[]` instead keeps
        // the calendar default a default.
        const remindersForWire = keepRemindersAsDefault
          ? []
          : form.reminders;

        // Notify attendees: gated identically to the toggle's visibility —
        // only when the target calendar can schedule server-side AND there
        // are attendees to notify.
        const targetCal = calendars.find((c) => c.id === form.calendarId);
        const sendInvitations =
          !!targetCal?.supports_scheduling &&
          form.attendees.length > 0 &&
          notifyAttendees;
        // When the target stores the color natively (local, or color-capable
        // CalDAV via RFC 7986 COLOR), apiCreate/UpdateEvent already carries it
        // on `color_label` — so the extra setEventColor call is only needed
        // for non-capable externals, where it writes a host-local override.
        const storesColorNatively =
          targetCal?.account_id === 'local' ||
          targetCal?.supports_event_color === true;

        // Whether the carry question has already taken over the dialog stack:
        // it closes this editor itself, so closing again below would pop the
        // question instead.
        let carriedToGroup = false;
        if (isEdit && event) {
          const seriesId = seriesIdOf(event);

          // Already an override — the user is editing an occurrence they have
          // edited before. There is nothing to carve out: the row in front of
          // them IS the exception, and the series already has its EXDATE. So
          // update it in place, by its OWN id.
          //
          // Without this branch it fell through to the series update below,
          // which writes to `seriesId` — and `seriesId` now resolves an
          // override to its master. "Just this one" would have rewritten the
          // whole series, which is the one outcome this dialog exists to
          // prevent.
          if (
            editScope === 'occurrence' &&
            !isExpandedOccurrence(event) &&
            occurrenceIsoOf(event) != null
          ) {
            const overrideRow: CalendarEvent = {
              ...event,
              title: trimmedTitle,
              calendar_id: form.calendarId,
              start,
              end,
              all_day: form.allDay,
              location: form.location.trim() || null,
              description: form.description.trim() || null,
              // Stays null: an override is one instance and owns no rule.
              recurrence: null,
              color_label: form.colorLabel,
              reminders: remindersForWire,
              attendees: form.attendees,
              send_invitations: sendInvitations,
            };
            await apiUpdateEvent(overrideRow, event.calendar_id);
            if (!storesColorNatively) {
              await setEventColor(
                overrideRow.id,
                overrideRow.calendar_id,
                form.colorLabel,
              );
            }
            announce(
              t('dialogs.event.occurrenceUpdated', { title: trimmedTitle }),
            );
            // The other copies have a series each, so carrying this means
            // carving the same occurrence out of them — not updating a row.
            // `offerToCarry` closes the editor itself when it asks; when it
            // stays silent nothing has closed yet.
            if (
              !(await offerToCarry(
                event,
                overrideRow,
                'occurrence',
                occurrenceIsoOf(event),
              ))
            ) {
              onClose();
            }
            return;
          }

          if (isOccurrence && editScope === 'occurrence' && event.recurrence) {
            // Single-instance override: add the original date to the
            // series EXDATE list, then create a standalone event with
            // the user's modified fields.
            const occIso = occurrenceIsoOf(event);
            if (occIso) {
              await addEventExdate(seriesId, occIso, event.calendar_id);
              const created = await apiCreateEvent({
                calendar_id: form.calendarId,
                title: trimmedTitle,
                description: form.description.trim() || null,
                location: form.location.trim() || null,
                start,
                end,
                all_day: form.allDay,
                recurrence: null,
                color_label: form.colorLabel,
                reminders: remindersForWire,
                sound: null,
                attendees: form.attendees,
                send_invitations: sendInvitations,
              });
              if (!storesColorNatively) {
                await setEventColor(
                  created.id,
                  created.calendar_id,
                  form.colorLabel,
                );
              }
              announce(
                t('dialogs.event.occurrenceUpdated', { title: trimmedTitle }),
              );
              if (!(await offerToCarry(event, created, 'occurrence', occIso))) {
                onClose();
              }
              return;
            }
          }

          // No `event.recurrence` precondition: this branch reads the rule
          // off the MASTER it loads below, and an override — which is exactly
          // the row a user splits a series at — carries none of its own.
          if (isOccurrence && editScope === 'this_and_future') {
            // Split the series at this occurrence: truncate the original to end
            // just before it (keeping its own fields), then create a NEW series
            // from here carrying the edits. The new series reuses the original
            // PATTERN (with the remaining COUNT); changing the recurrence pattern
            // itself for "this and following" isn't supported — edit the whole
            // series for that.
            const occIso = occurrenceIsoOf(event);
            // Pass the owning calendar so an EXTERNAL master resolves via the SWR
            // cache. A null master (cold cache) is a hard error, never a silent
            // fall-through to a whole-series edit that would move every occurrence.
            const master = await getEventById(seriesId, event.calendar_id);
            if (occIso && master == null) {
              throw new Error(
                t('dialogs.event.thisAndFutureLoadFailed', {
                  title: event.title,
                }),
              );
            }
            const masterRecurrence = master?.recurrence ?? null;
            const plan = master ? planSeriesSplit(master, occIso ?? '') : null;
            if (occIso && master && masterRecurrence && plan) {
              // The arithmetic — the COUNT the tail keeps, the EXDATEs that
              // travel with it, the zone it inherits — lives in
              // `planSeriesSplit`; the order and the recovery in
              // `writeSeriesSplit`. See shared/seriesSplit.ts for why each of
              // those details decides whether the two halves line up.
              const created = await writeSeriesSplit(
                {
                  // Notify on the truncate too (symmetric with delete-this-and-
                  // following): on notify-flag providers attendees must be told
                  // the original series now ends before the cutoff, or they keep
                  // the old occurrences AND get the new tail invite.
                  truncate: (headRule) =>
                    apiUpdateEvent(
                      {
                        ...master,
                        recurrence: { ...masterRecurrence, rrule: headRule },
                        send_invitations: sendInvitations,
                        truncate_tail_overrides: true,
                      },
                      master.calendar_id,
                    ),
                  createTail: (recurrence) =>
                    apiCreateEvent(
                      {
                        calendar_id: form.calendarId,
                        title: trimmedTitle,
                        description: form.description.trim() || null,
                        location: form.location.trim() || null,
                        start,
                        end,
                        all_day: form.allDay,
                        recurrence,
                        color_label: form.colorLabel,
                        reminders: remindersForWire,
                        sound: null,
                        attendees: form.attendees,
                        send_invitations: sendInvitations,
                      },
                      // Continuation of the master — keep its zone verbatim
                      // (incl. floating) so head and tail expand identically.
                      { preserveRecurrenceZone: true },
                    ),
                  restore: () =>
                    apiUpdateEvent(
                      { ...master, send_invitations: sendInvitations },
                      master.calendar_id,
                    ),
                },
                plan,
              );
              if (!storesColorNatively) {
                await setEventColor(
                  created.id,
                  created.calendar_id,
                  form.colorLabel,
                );
              }
              announce(
                t('dialogs.event.thisAndFutureUpdated', { title: trimmedTitle }),
              );
              // The other copies have a series each, so carrying this means
              // splitting theirs at the same point — not updating a row.
              if (!(await offerToCarry(event, created, 'future', occIso))) {
                onClose();
              }
              return;
            }
          }

          const updated: CalendarEvent = {
            ...event,
            id: seriesId,
            title: trimmedTitle,
            calendar_id: form.calendarId,
            start,
            end,
            all_day: form.allDay,
            location: form.location.trim() || null,
            description: form.description.trim() || null,
            recurrence,
            color_label: form.colorLabel,
            reminders: remindersForWire,
            attendees: form.attendees,
            send_invitations: sendInvitations,
          };
          // Thread the *original* calendar_id so the backend can
          // tell an in-place edit (Save with the picker untouched)
          // apart from a calendar-change move (Save with the picker
          // pointing somewhere new). Without this hint, the latter
          // would PUT to a resource that doesn't exist on the new
          // calendar — iCloud 412s the precondition because the
          // old etag's If-Match can never be satisfied at the new
          // URL. The backend takes the hint and reroutes the
          // change as a create-on-target + delete-from-source.
          await apiUpdateEvent(updated, event.calendar_id);
          // Color rides update_event for local + color-capable calendars; only
          // a non-capable external needs the separate host-local override.
          if (!storesColorNatively) {
            await setEventColor(
              updated.id,
              updated.calendar_id,
              form.colorLabel,
            );
          }
          announce(t('dialogs.event.updated', { title: trimmedTitle }));
          // The appointment may exist several times over. Ask — after the
          // save, so the user's own change is never at stake — whether the
          // other copies should follow (DESIGN-event-groups.md, Stufe 2).
          // Only for a whole-event edit: an occurrence override and a
          // series truncation each return above, because "which copy of
          // which occurrence" is a question this cannot answer yet.
          carriedToGroup = await offerToCarry(event, updated);
        } else {
          const created = await apiCreateEvent({
            calendar_id: form.calendarId,
            title: trimmedTitle,
            description: form.description.trim() || null,
            location: form.location.trim() || null,
            start,
            end,
            all_day: form.allDay,
            recurrence,
            color_label: form.colorLabel,
            reminders: remindersForWire,
            sound: null,
            attendees: form.attendees,
            send_invitations: sendInvitations,
          });
          if (!storesColorNatively) {
            await setEventColor(
              created.id,
              created.calendar_id,
              form.colorLabel,
            );
          }
          // Remember the calendar for the next new-event open. Only
          // for *creates*: edits shouldn't bias future picks, since
          // the user might have only changed a recurring event in
          // a calendar they don't usually write to.
          writeLastUsedCalendar(form.calendarId);
          announce(t('dialogs.event.created', { title: trimmedTitle }));
        }
        if (!carriedToGroup) onClose();
      } catch (err) {
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      } finally {
        setSubmitting(false);
      }
    },
    [
      form,
      submitting,
      isEdit,
      event,
      isOccurrence,
      editScope,
      keepRemindersAsDefault,
      calendars,
      notifyAttendees,
      announce,
      offerToCarry,
      onClose,
      t,
    ],
  );

  // A meeting you organize (with attendees) offers "cancel + notify" vs
  // "remove silently"; everything else is a plain delete. The same choice is
  // offered for a whole meeting/series and for a single occurrence — `scope`
  // routes the dialog's confirm to the right removal.
  const { offersChoice } = useCancellationChoice(event);
  const [cancelChoiceOpen, setCancelChoiceOpen] = useState(false);
  const [cancelChoiceScope, setCancelChoiceScope] = useState<
    'series' | 'occurrence' | 'this_and_future'
  >('series');

  // The actual removal, parameterised by whether the provider should email a
  // cancellation to the attendees. Used both by the plain-delete path and the
  // cancel-choice dialog.
  const performDelete = useCallback(
    async (sendCancellations: boolean) => {
      if (!event) return;
      setError(null);
      setSubmitting(true);
      try {
        const seriesId = seriesIdOf(event);
        await deleteEventById(seriesId, event.calendar_id, sendCancellations);
        announce(
          sendCancellations
            ? t('dialogs.event.meetingCancelled', { title: event.title })
            : t('dialogs.event.deleted', { title: event.title }),
        );
        onClose();
      } catch (err) {
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      } finally {
        setSubmitting(false);
      }
    },
    [event, announce, onClose, t],
  );

  // Remove a single occurrence of a recurring series. When `sendCancellations`
  // is true (organizer chose "cancel + notify") the provider emails attendees a
  // cancellation for just this occurrence; otherwise it's a silent local skip.
  const performOccurrenceDelete = useCallback(
    async (sendCancellations: boolean) => {
      if (!event) return;
      const occIso = occurrenceIsoOf(event);
      if (!occIso) return;
      setError(null);
      setSubmitting(true);
      try {
        await addEventExdate(
          seriesIdOf(event),
          occIso,
          event.calendar_id,
          sendCancellations,
        );
        announce(
          sendCancellations
            ? t('dialogs.event.occurrenceCancelled', { title: event.title })
            : t('dialogs.event.occurrenceDeleted', { title: event.title }),
        );
        onClose();
      } catch (err) {
        setError(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      } finally {
        setSubmitting(false);
      }
    },
    [event, announce, onClose, t],
  );

  // "Delete this and all following": truncate the series to end before this
  // occurrence. `sendCancellations` notifies attendees of the change.
  const performThisAndFutureDelete = useCallback(
    async (sendCancellations: boolean) => {
      if (!event) return;
      const occIso = occurrenceIsoOf(event);
      if (!occIso) return;
      setError(null);
      setSubmitting(true);
      try {
        await deleteThisAndFuture(event, occIso, sendCancellations);
        announce(
          sendCancellations
            ? t('dialogs.event.thisAndFutureCancelled', { title: event.title })
            : t('dialogs.event.thisAndFutureDeleted', { title: event.title }),
        );
        onClose();
      } catch (err) {
        setError(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      } finally {
        setSubmitting(false);
      }
    },
    [event, announce, onClose, t],
  );

  const onDelete = useCallback(async () => {
    if (!event) return;
    if (submitting) return;
    // Removing a single occurrence of a recurring event.
    if (isOccurrence && editScope === 'occurrence' && event.recurrence) {
      // Organizer with attendees → offer "cancel this occurrence + notify" vs
      // silent local skip; otherwise a plain EXDATE (no attendee notification).
      if (offersChoice) {
        setCancelChoiceScope('occurrence');
        setCancelChoiceOpen(true);
        return;
      }
      await performOccurrenceDelete(false);
      return;
    }
    // Removing this occurrence and all following ones (truncate the series).
    if (isOccurrence && editScope === 'this_and_future' && event.recurrence) {
      if (offersChoice) {
        setCancelChoiceScope('this_and_future');
        setCancelChoiceOpen(true);
        return;
      }
      await performThisAndFutureDelete(false);
      return;
    }
    // Organizer removing a whole meeting/series with attendees → ask whether to notify.
    if (offersChoice) {
      setCancelChoiceScope('series');
      setCancelChoiceOpen(true);
      return;
    }
    await performDelete(false);
  }, [
    event,
    submitting,
    isOccurrence,
    editScope,
    offersChoice,
    performDelete,
    performOccurrenceDelete,
    performThisAndFutureDelete,
  ]);

  const title = isEdit ? t('dialogs.event.editTitle') : t('dialogs.event.newTitle');

  // Birthday events (DESIGN.md §10.3) are synthesised from
  // contacts. The full edit form would let the user type into
  // fields whose save would fail at the backend — better to
  // short-circuit to a read-only summary the user can dismiss.
  // The contact source is what they edit instead.
  if (event && isBirthdayEventId(event.id)) {
    return (
      <Modal
        isOpen={isOpen}
        onClose={onClose}
        title={t('dialogs.event.birthdayTitle')}
        className="modal--form modal--birthday"
      >
        <div className="form">
          <FocusableNote className="modal-birthday__name">
            {event.title}
          </FocusableNote>
          {event.description && (
            <FocusableNote className="form__hint">
              {t('dialogs.event.birthdayAge', { age: event.description })}
            </FocusableNote>
          )}
          <FocusableNote className="form__hint">
            {t('dialogs.event.birthdayHint')}
          </FocusableNote>
          <div className="form__actions">
            <button
              type="button"
              className="form__action form__action--primary"
              onClick={onClose}
            >
              {t('dialogs.event.birthdayClose')}
            </button>
          </div>
        </div>
      </Modal>
    );
  }

  return (
    <>
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      className="modal--form"
      dismissOnBackdrop={false}
    >
      <form onSubmit={onSubmit} className="form">
        {isEdit && event && (
          <EventRsvp event={event} onResponded={onClose} />
        )}
        <TitleSuggestBox
          label={t('dialogs.event.fields.title')}
          value={form.title}
          onChange={(v) => update('title', v)}
          options={titleOptions}
          onAccept={acceptTitleSuggestion}
          required
        />

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.calendar')}
          </span>
          <select
            value={form.calendarId}
            onChange={(e) => update('calendarId', e.target.value)}
            required
          >
            <option value="" disabled>
              {t('dialogs.event.pickCalendar')}
            </option>
            {/* Offer only calendars the user can write to AND still shows in
                the sidebar (`selectableEventCalendars`): a read-only feed
                rejects new events, and a hidden calendar is a confusing
                target. The event being edited might itself live on a read-only
                or hidden calendar — `currentId` keeps it so the picker still
                matches `form.calendarId`. */}
            {selectableEventCalendars(calendars, {
              selectedIds: selectedCalendarIds,
              currentId: form.calendarId,
              includeHidden: showHiddenCalendarTargets,
            }).map((cal) => (
              <option key={cal.id} value={cal.id}>
                {cal.name}
              </option>
            ))}
          </select>
        </label>

        <label className="form__field form__field--inline">
          <input
            type="checkbox"
            checked={form.allDay}
            onChange={(e) => update('allDay', e.target.checked)}
          />
          <span>{t('dialogs.event.fields.allDay')}</span>
        </label>

        <div className="form__row">
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.fields.startDate')}
            </span>
            <input
              type="date"
              value={form.startDate}
              onChange={(e) => update('startDate', e.target.value)}
              required
            />
          </label>
          {!form.allDay && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.fields.startTime')}
              </span>
              <input
                type="time"
                value={form.startTime}
                onChange={(e) => update('startTime', e.target.value)}
                required
              />
            </label>
          )}
        </div>

        <div className="form__row">
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.fields.endDate')}
            </span>
            <input
              type="date"
              value={form.endDate}
              onChange={(e) => update('endDate', e.target.value)}
              required
            />
          </label>
          {!form.allDay && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.event.fields.endTime')}
              </span>
              <input
                type="time"
                value={form.endTime}
                onChange={(e) => update('endTime', e.target.value)}
                required
              />
            </label>
          )}
        </div>

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.location')}
          </span>
          <input
            type="text"
            value={form.location}
            onChange={(e) => update('location', e.target.value)}
            autoComplete="off"
          />
        </label>

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.description')}
          </span>
          <textarea
            value={form.description}
            onChange={(e) => update('description', e.target.value)}
            rows={4}
          />
        </label>
        <DescriptionLinks text={form.description} />
        {/* Any conference in this event, whoever created it — an Outlook or
            eM Client invitation as readily as one Aperio made. Detection is
            shared with the mobile app and reads URLs rather than prose, so it
            does not depend on the invitation's language. */}
        <ConferenceSection
          location={form.location}
          description={form.description}
        />
        {/* Creating one, as opposed to joining one. Only for a meeting Aperio
            owns — an event carrying someone else's link gets Join above and no
            Remove here, because it is not ours to delete. */}
        <MeetingControls
          event={event ?? null}
          onEventChanged={(saved) => {
            update('location', saved.location ?? '');
            update('description', saved.description ?? '');
          }}
        />

        <div className="form__field">
          <span className="form__label" id={attendeesLabelId}>
            {t('dialogs.event.fields.attendees')}
          </span>
          <AttendeePicker
            value={form.attendees}
            onChange={(next) => update('attendees', next)}
            labelledBy={attendeesLabelId}
          />
        </div>

        {calendars.find((c) => c.id === form.calendarId)
          ?.supports_scheduling &&
          form.attendees.length > 0 && (
            <>
              <label className="form__field form__field--inline">
                <input
                  type="checkbox"
                  checked={notifyAttendees}
                  onChange={(e) => setNotifyAttendees(e.target.checked)}
                />
                <span>{t('dialogs.event.fields.notifyAttendees')}</span>
              </label>

              <div className="form__field availability">
                <button
                  type="button"
                  className="form__action availability__check"
                  onClick={checkAvailability}
                  disabled={checkingAvailability}
                >
                  {checkingAvailability
                    ? t('dialogs.event.availability.checking')
                    : t('dialogs.event.availability.check')}
                </button>
                {availabilityError && (
                  <p className="form__error" role="alert">
                    {availabilityError}
                  </p>
                )}
                {availability && availabilityWindow && (
                  <div className="availability__results" role="status">
                    <p className="availability__summary">
                      {availability.filter((fb) =>
                        isBusyInWindow(
                          fb.slots,
                          availabilityWindow.start,
                          availabilityWindow.end,
                        ),
                      ).length === 0
                        ? t('dialogs.event.availability.allFree')
                        : t('dialogs.event.availability.someBusy', {
                            count: availability.filter((fb) =>
                              isBusyInWindow(
                                fb.slots,
                                availabilityWindow.start,
                                availabilityWindow.end,
                              ),
                            ).length,
                          })}
                    </p>
                    <ul className="availability__list">
                      {availability.map((fb) => {
                        const busy = isBusyInWindow(
                          fb.slots,
                          availabilityWindow.start,
                          availabilityWindow.end,
                        );
                        return (
                          <li
                            key={fb.email}
                            className={
                              busy
                                ? 'availability__item availability__item--busy'
                                : 'availability__item availability__item--free'
                            }
                          >
                            <span className="availability__email">
                              {fb.email}
                            </span>
                            <span className="availability__status">
                              {busy
                                ? t('dialogs.event.availability.busy')
                                : t('dialogs.event.availability.free')}
                            </span>
                          </li>
                        );
                      })}
                    </ul>
                  </div>
                )}
              </div>
            </>
          )}

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.colorLabel')}
          </span>
          <ColorLabelSelect
            value={form.colorLabel}
            onChange={(next) => update('colorLabel', next)}
            labels={colorLabels}
            noneLabel={t('dialogs.event.noColorLabel')}
          />
        </label>

        <RecurrenceSelector
          value={form.rrule}
          onChange={(rrule) => update('rrule', rrule)}
          start={recurrenceStartDate(form.startDate)}
          capabilities={
            calendars.find((c) => c.id === form.calendarId)
              ?.recurrence_capabilities
          }
        />

        <RemindersEditor
          value={form.reminders}
          onChange={(next) => {
            update('reminders', next);
            // The moment the user touches the editor, the entries
            // become real per-event reminders — clear the
            // "keep as default" gate so submit actually sends them.
            setKeepRemindersAsDefault(false);
          }}
          mode="event"
        />

        {/* §14.4 per-event sound override. Edit-only: the key is the
            event's (series) id, which a not-yet-created event doesn't
            have. New events inherit the calendar / global default. */}
        {isEdit && event && (
          <SoundPrefField prefKey={`sound.item.${seriesIdOf(event)}`} />
        )}

        {/* The recurring-edit scope is normally chosen in the up-front
            prompt (see EditEventScopeDialog), so the editor just confirms it
            read-only — one clear choice beats a radio group a screen-reader
            user could miss. The radios remain as a fallback for any path that
            opens an occurrence without going through the prompt. */}
        {isOccurrence && initialScope != null && (
          <p className="form__hint">
            {t('dialogs.event.scope.label')}:{' '}
            {t(
              editScope === 'occurrence'
                ? 'dialogs.event.scope.occurrence'
                : 'dialogs.event.scope.series',
            )}
          </p>
        )}
        {isOccurrence && initialScope == null && (
          <fieldset className="form__field">
            <legend className="form__label">
              {t('dialogs.event.scope.label')}
            </legend>
            <label className="form__field form__field--inline">
              <input
                type="radio"
                name="event-scope"
                checked={editScope === 'occurrence'}
                onChange={() => setEditScope('occurrence')}
              />
              <span>{t('dialogs.event.scope.occurrence')}</span>
            </label>
            <label className="form__field form__field--inline">
              <input
                type="radio"
                name="event-scope"
                checked={editScope === 'this_and_future'}
                onChange={() => setEditScope('this_and_future')}
              />
              <span>{t('dialogs.event.scope.thisAndFuture')}</span>
            </label>
            <label className="form__field form__field--inline">
              <input
                type="radio"
                name="event-scope"
                checked={editScope === 'series'}
                onChange={() => setEditScope('series')}
              />
              <span>{t('dialogs.event.scope.series')}</span>
            </label>
          </fieldset>
        )}

        {isEdit && event?.recurrence && !isOccurrence && (
          <p className="form__hint">
            {t('dialogs.event.recurrence.editsSeries')}
          </p>
        )}

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <div className="form__actions">
          {isEdit && (
            <button
              type="button"
              onClick={onDelete}
              aria-disabled={submitting || undefined}
              className="form__action form__action--danger"
            >
              {t('dialogs.event.delete')}
            </button>
          )}
          <button
            type="button"
            // aria-disabled (matching the delete button above) instead of native
            // disabled: natively disabling the focused Save/Cancel button blurs
            // focus to <body> for the whole save round-trip and strands it there
            // when the save fails. The onClick guard preserves "can't cancel
            // mid-save".
            onClick={() => {
              if (!submitting) onClose();
            }}
            aria-disabled={submitting || undefined}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            // aria-disabled keeps focus on Save through the round-trip; the
            // onSubmit re-entry guard (`if (submitting) return`) already blocks
            // a double PUT, so no native disable is needed.
            aria-disabled={submitting || undefined}
            className="form__action form__action--primary"
          >
            {isEdit ? t('dialogs.save') : t('dialogs.create')}
          </button>
        </div>
      </form>
    </Modal>
    {event && (
      <ConfirmDialog
        isOpen={cancelChoiceOpen}
        onClose={() => setCancelChoiceOpen(false)}
        title={t('dialogs.event.cancelChoice.title')}
        message={t(
          cancelChoiceScope === 'occurrence'
            ? 'dialogs.event.cancelChoice.occurrenceMessage'
            : cancelChoiceScope === 'this_and_future'
              ? 'dialogs.event.cancelChoice.thisAndFutureMessage'
              : 'dialogs.event.cancelChoice.message',
          { title: event.title },
        )}
        confirmLabel={t(
          cancelChoiceScope === 'occurrence'
            ? 'dialogs.event.cancelChoice.cancelOccurrence'
            : cancelChoiceScope === 'this_and_future'
              ? 'dialogs.event.cancelChoice.cancelThisAndFuture'
              : 'dialogs.event.cancelChoice.cancelMeeting',
        )}
        onConfirm={() => {
          if (cancelChoiceScope === 'occurrence') {
            void performOccurrenceDelete(true);
          } else if (cancelChoiceScope === 'this_and_future') {
            void performThisAndFutureDelete(true);
          } else {
            void performDelete(true);
          }
        }}
        extraActions={[
          {
            label: t('dialogs.event.cancelChoice.removeSilently'),
            onClick: () => {
              if (cancelChoiceScope === 'occurrence') {
                void performOccurrenceDelete(false);
              } else if (cancelChoiceScope === 'this_and_future') {
                void performThisAndFutureDelete(false);
              } else {
                void performDelete(false);
              }
            },
            danger: true,
          },
        ]}
      />
    )}
    </>
  );
}

function buildInitialState(
  event: CalendarEvent | null,
  defaultCalendarId: string | undefined,
  defaultDate: string | undefined,
  defaultTime: string | undefined,
  defaultTitle: string | undefined,
  calendars: { id: string; read_only: boolean }[],
  selectedIds: ReadonlySet<string>,
): FormState {
  if (event) {
    const start = new Date(event.start);
    const end = new Date(event.end);
    return {
      title: event.title,
      calendarId: event.calendar_id,
      startDate: dateInput(start),
      startTime: timeInput(start),
      // All-day ends are stored EXCLUSIVE (last day + 1); the form's
      // end-date input shows the last covered day. Legacy inclusive
      // rows (end == start) clamp to a valid single-day range.
      endDate: event.all_day
        ? allDayFormEndDate(start, end)
        : dateInput(end),
      endTime: timeInput(end),
      allDay: event.all_day,
      location: event.location ?? '',
      description: event.description ?? '',
      rrule: event.recurrence?.rrule ?? null,
      colorLabel: event.color_label ?? null,
      reminders: event.reminders ?? [],
      attendees: event.attendees ?? [],
    };
  }

  // New event: a 1-hour slot whose time-of-day adapts to context:
  //  - anchored on today       → next :00/:30 slot (just ahead of now)
  //  - anchored on another day → 09:00 (start of the workday)
  //  - no anchor at all        → next full hour from now
  // The anchor is normally the day the active view is focused on,
  // threaded in by the caller as `defaultDate`.
  let { start, end } = defaultNewEventTimes(defaultDate, new Date());
  // …but if the caller carried a picked start time over (the quick-add's
  // "weitere Details" hand-off), honour it instead of the derived slot, keeping
  // the 1-hour duration.
  if (defaultTime && /^\d{2}:\d{2}$/.test(defaultTime)) {
    const [h, m] = defaultTime.split(':').map(Number);
    const picked = new Date(start);
    picked.setHours(h, m, 0, 0);
    start = picked;
    end = new Date(picked.getTime() + 60 * 60 * 1000);
  }

  // Fallback chain for the calendar dropdown on a *new* event:
  //   1. explicit `defaultCalendarId` from the caller (e.g. when the
  //      user pressed Enter on a specific calendar's row)
  //   2. last-used calendar (persisted in localStorage on every
  //      successful create) — provided it still exists and isn't
  //      read-only
  //   3. first writable calendar
  //   4. first calendar regardless of read-only-ness (degenerate case
  //      where the only available calendar is a subscription feed; the
  //      dropdown filter will still hide it and the submit blocks)
  // Prefer a calendar the user can create in AND still shows in the sidebar —
  // the same set `selectableEventCalendars` offers in the dropdown.
  const selectable = selectableEventCalendars(calendars, { selectedIds });
  const lastUsed = readLastUsedCalendar();
  const lastUsedIfValid =
    lastUsed && selectable.some((c) => c.id === lastUsed) ? lastUsed : null;
  const fallbackCalendar =
    defaultCalendarId ??
    lastUsedIfValid ??
    selectable[0]?.id ??
    calendars[0]?.id ??
    '';

  return {
    title: defaultTitle ?? '',
    calendarId: fallbackCalendar,
    startDate: dateInput(start),
    startTime: timeInput(start),
    endDate: dateInput(end),
    endTime: timeInput(end),
    allDay: false,
    location: '',
    description: '',
    rrule: null,
    colorLabel: null,
    reminders: [],
    attendees: [],
  };
}
