import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { selectableEventCalendars } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import { DescriptionLinks } from './DescriptionLinks';
import {
  addEventExdate,
  createEvent as apiCreateEvent,
  deleteEventById,
  isCommandError,
  queryFreeBusy,
  setEventColor,
  updateEvent as apiUpdateEvent,
} from '../api/client';
import type { CalendarEvent, FreeBusy, FreeBusySlot } from '../api/types';
import {
  isExpandedOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../intl/recurrence';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import { useCancellationChoice } from '../state/useCancellationChoice';
import { useViewState } from '../state/viewStateContext';
import { AttendeePicker } from './AttendeePicker';
import { ConfirmDialog } from './ConfirmDialog';
import { ColorLabelSelect } from './ColorLabelSelect';
import {
  allDayFormEndDate,
  allDayWireEnd,
  applyDateTimeChange,
  dateInput,
  defaultNewEventTimes,
  recurrenceStartDate,
  timeInput,
  toIso,
} from './eventDateTime';
import { EventRsvp } from './EventRsvp';
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
  /** Pre-fill the title when creating — carries the in-progress title over
   *  from the event quick-add's "weitere Details" hand-off. Ignored when
   *  editing. */
  defaultTitle?: string;
  /** When editing a recurring occurrence, the scope the up-front prompt
   *  resolved to. Seeds the editScope radios; absent ⇒ 'occurrence'. */
  initialScope?: EditScope;
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
type EditScope = 'series' | 'occurrence';

/** True when the id carries the synthetic `@ISO` suffix from `expandEvent`. */
export function EventDialog({
  isOpen,
  onClose,
  event,
  defaultCalendarId,
  defaultDate,
  defaultTitle,
  initialScope,
}: EventDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
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
    defaultTitle,
    calendars,
    selectedCalendarIds,
    remindersWereFromDefault,
    getDefaultsFor,
  ]);

  const [form, setForm] = useState<FormState>(initialState);
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
  const isOccurrence = isEdit && !!event && isExpandedOccurrence(event);
  const [editScope, setEditScope] = useState<EditScope>(
    initialScope ?? 'occurrence',
  );

  // Reset the form whenever the dialog is opened with new context. We
  // key on isOpen + initialState; isOpen=false keeps the previous form
  // around briefly while the close animation runs (if we ever add one).
  useEffect(() => {
    if (isOpen) {
      setForm(initialState);
      setError(null);
      setEditScope(initialScope ?? 'occurrence');
      // Re-arm the "keep defaults out of the wire" flag every time
      // the dialog (re-)opens with a fresh event.
      setKeepRemindersAsDefault(remindersWereFromDefault);
      setNotifyAttendees(true);
      setAvailability(null);
      setAvailabilityWindow(null);
      setAvailabilityError(null);
    }
  }, [isOpen, initialState, remindersWereFromDefault, initialScope]);

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

        if (isEdit && event) {
          const seriesId = seriesIdOf(event);

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
              onClose();
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
    'series' | 'occurrence'
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
  ]);

  const title = isEdit ? t('dialogs.event.editTitle') : t('dialogs.event.newTitle');

  // Birthday events (DESIGN.md §10.3) are synthesised from
  // contacts. The full edit form would let the user type into
  // fields whose save would fail at the backend — better to
  // short-circuit to a read-only summary the user can dismiss.
  // The contact source is what they edit instead.
  if (event && event.id.startsWith('aperio-birthday:')) {
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
        <label className="form__field">
          <span className="form__label">{t('dialogs.event.fields.title')}</span>
          <input
            type="text"
            value={form.title}
            onChange={(e) => update('title', e.target.value)}
            required
            autoComplete="off"
          />
        </label>

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
            onClick={onClose}
            disabled={submitting}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            disabled={submitting}
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
        message={
          cancelChoiceScope === 'occurrence'
            ? t('dialogs.event.cancelChoice.occurrenceMessage', {
                title: event.title,
              })
            : t('dialogs.event.cancelChoice.message', { title: event.title })
        }
        confirmLabel={
          cancelChoiceScope === 'occurrence'
            ? t('dialogs.event.cancelChoice.cancelOccurrence')
            : t('dialogs.event.cancelChoice.cancelMeeting')
        }
        onConfirm={() =>
          cancelChoiceScope === 'occurrence'
            ? void performOccurrenceDelete(true)
            : void performDelete(true)
        }
        extraActions={[
          {
            label: t('dialogs.event.cancelChoice.removeSilently'),
            onClick: () =>
              cancelChoiceScope === 'occurrence'
                ? void performOccurrenceDelete(false)
                : void performDelete(false),
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
  const { start, end } = defaultNewEventTimes(defaultDate, new Date());

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
