import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

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
}: {
  isOpen: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars } = useCalendarStore();
  const { anchor } = useViewState();
  const { openEventDialog } = useDialogState();

  const initial = useMemo(
    () => buildInitial(calendars, anchor),
    [calendars, anchor],
  );

  const [title, setTitle] = useState(initial.title);
  const [date, setDate] = useState(initial.date);
  const [time, setTime] = useState(initial.time);
  const [calendarId, setCalendarId] = useState(initial.calendarId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setTitle(initial.title);
      setDate(initial.date);
      setTime(initial.time);
      setCalendarId(initial.calendarId);
      setError(null);
    }
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
    onClose();
    openEventDialog(null, {
      calendarId: calendarId || undefined,
      defaultDate: date || undefined,
    });
  }, [onClose, openEventDialog, calendarId, date]);

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
            {calendars.map((cal) => (
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
): Initial {
  // Same default-time policy as the full dialog: next :00/:30 slot when
  // the focused day is today, 09:00 otherwise.
  const { start } = defaultNewEventTimes(dateInput(anchor), new Date());
  return {
    title: '',
    date: dateInput(start),
    time: timeInput(start),
    calendarId: calendars[0]?.id ?? '',
  };
}
