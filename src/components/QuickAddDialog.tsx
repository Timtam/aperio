import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { selectableEventCalendars } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { createEvent as apiCreateEvent, isCommandError } from '../api/client';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import { useViewState } from '../state/viewStateContext';
import {
  dateInput,
  defaultNewEventTimes,
  timeInput,
  toIso,
} from './eventDateTime';
import { Modal } from './Modal';

/**
 * Quick-add dialog (DESIGN.md section 7.4).
 *
 * The minimal three-field form for one-tap event creation: title,
 * date+time, and calendar. The "More details" button swaps the current
 * dialog with the full EventDialog, carrying the in-progress form values
 * along so nothing is lost.
 */
export function QuickAddDialog({
  isOpen,
  onClose,
  defaultDate,
}: {
  isOpen: boolean;
  onClose: () => void;
  /** YYYY-MM-DD to anchor the new event on (an activated calendar day).
   *  When omitted the dialog falls back to the view's focused day. */
  defaultDate?: string;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars, selectedCalendarIds } = useCalendarStore();
  const { anchor, showHiddenCalendarTargets } = useViewState();
  const { openEventDialog } = useDialogState();

  // Writable calendars that can host a new event: the sidebar-visible ones, plus
  // hidden ones when the "show hidden as targets" pref is on.
  const selectable = useMemo(
    () =>
      selectableEventCalendars(calendars, {
        selectedIds: selectedCalendarIds,
        includeHidden: showHiddenCalendarTargets,
      }),
    [calendars, selectedCalendarIds, showHiddenCalendarTargets],
  );
  const initial = useMemo(
    () => buildInitial(selectable, anchor, defaultDate),
    [selectable, anchor, defaultDate],
  );

  const [title, setTitle] = useState(initial.title);
  const [date, setDate] = useState(initial.date);
  const [time, setTime] = useState(initial.time);
  const [calendarId, setCalendarId] = useState(initial.calendarId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // Live mirrors for the pristine check — refs so the reset effect reads the
  // CURRENT fields without listing them as deps (which would defeat the guard).
  const titleRef = useRef(title);
  titleRef.current = title;
  const dateRef = useRef(date);
  dateRef.current = date;
  const timeRef = useRef(time);
  timeRef.current = time;
  const calendarIdRef = useRef(calendarId);
  calendarIdRef.current = calendarId;
  const appliedInitialRef = useRef<Initial | null>(null);

  // `initial` is a useMemo over `selectable` (calendars + selection), so it
  // re-derives identity on every background catalog refresh while the dialog is
  // open. Re-applying it then wiped the title the user was typing. Adopt a fresh
  // `initial` only while the form is still PRISTINE (equal to the last one we
  // applied) — this still lets a late-arriving default calendar populate on a
  // cold open, but once the user types, their input wins until the dialog closes.
  useEffect(() => {
    if (!isOpen) {
      appliedInitialRef.current = null;
      return;
    }
    const baseline = appliedInitialRef.current;
    const pristine =
      baseline === null ||
      (titleRef.current === baseline.title &&
        dateRef.current === baseline.date &&
        timeRef.current === baseline.time &&
        calendarIdRef.current === baseline.calendarId);
    if (!pristine) return; // user touched the form → their input wins
    appliedInitialRef.current = initial;
    setTitle(initial.title);
    setDate(initial.date);
    setTime(initial.time);
    setCalendarId(initial.calendarId);
    setError(null);
  }, [isOpen, initial]);

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      const trimmed = title.trim();
      if (!trimmed) {
        setError(t('dialogs.event.titleRequired'));
        return;
      }
      if (!calendarId) {
        setError(t('dialogs.event.calendarRequired'));
        return;
      }
      const start = toIso(date, time, false);
      if (!start) {
        setError(t('dialogs.event.dateInvalid'));
        return;
      }
      const end = new Date(new Date(start).getTime() + 60 * 60 * 1000).toISOString();

      setSubmitting(true);
      try {
        await apiCreateEvent({
          calendar_id: calendarId,
          title: trimmed,
          description: null,
          location: null,
          start,
          end,
          all_day: false,
          recurrence: null,
          color_label: null,
          reminders: [],
          sound: null,
          attendees: [],
        });
        announce(t('dialogs.event.created', { title: trimmed }));
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
    [title, calendarId, date, time, announce, onClose, t],
  );

  const openFullDialog = useCallback(() => {
    // Hand off by REPLACING the quick-add frame (not close()+push): the editor
    // then inherits the quick-add's trigger, so closing it returns focus to the
    // calendar grid / day the user activated — not the view's first focusable.
    openEventDialog(null, {
      calendarId: calendarId || undefined,
      defaultDate: date || undefined,
      // Carry the in-progress title over so it isn't lost on the hand-off.
      defaultTitle: title || undefined,
      replace: true,
    });
  }, [openEventDialog, calendarId, date, title]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.quickAdd.title')}
      className="modal--form modal--narrow"
      dismissOnBackdrop={false}
    >
      <form onSubmit={onSubmit} className="form">
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.title')}
          </span>
          <input
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            required
            autoComplete="off"
          />
        </label>

        <div className="form__row">
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.fields.startDate')}
            </span>
            <input
              type="date"
              value={date}
              onChange={(e) => setDate(e.target.value)}
              required
            />
          </label>
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.event.fields.startTime')}
            </span>
            <input
              type="time"
              value={time}
              onChange={(e) => setTime(e.target.value)}
              required
            />
          </label>
        </div>

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.calendar')}
          </span>
          <select
            value={calendarId}
            onChange={(e) => setCalendarId(e.target.value)}
            required
          >
            <option value="" disabled>
              {t('dialogs.event.pickCalendar')}
            </option>
            {selectable.map((cal) => (
              <option key={cal.id} value={cal.id}>
                {cal.name}
              </option>
            ))}
          </select>
        </label>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <div className="form__actions">
          <button
            type="button"
            onClick={openFullDialog}
            aria-disabled={submitting || undefined}
            className="form__action"
          >
            {t('dialogs.quickAdd.moreDetails')}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-disabled={submitting || undefined}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            aria-disabled={submitting || undefined}
            className="form__action form__action--primary"
          >
            {t('dialogs.create')}
          </button>
        </div>
      </form>
    </Modal>
  );
}

interface Initial {
  title: string;
  date: string;
  time: string;
  calendarId: string;
}

function buildInitial(
  calendars: { id: string }[],
  anchor: Date,
  defaultDate?: string,
): Initial {
  // Same default-time policy as the full dialog: next :00/:30 slot when
  // the focused day is today, 09:00 otherwise. An activated calendar day
  // (`defaultDate`) overrides the view's focused day when present.
  const { start } = defaultNewEventTimes(
    defaultDate || dateInput(anchor),
    new Date(),
  );
  return {
    title: '',
    date: dateInput(start),
    time: timeInput(start),
    calendarId: calendars[0]?.id ?? '',
  };
}
