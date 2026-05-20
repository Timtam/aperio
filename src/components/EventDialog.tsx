import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  addEventExdate,
  createEvent as apiCreateEvent,
  deleteEventById,
  isCommandError,
  updateEvent as apiUpdateEvent,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { Modal } from './Modal';
import { RecurrenceSelector } from './RecurrenceSelector';

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
function isOccurrenceId(id: string): boolean {
  return id.includes('@');
}

/** Extract the original ISO start from a synthetic occurrence id. */
function occurrenceIsoFromId(id: string): string | null {
  const idx = id.indexOf('@');
  return idx >= 0 ? id.slice(idx + 1) : null;
}

export function EventDialog({
  isOpen,
  onClose,
  event,
  defaultCalendarId,
  defaultDate,
}: EventDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars, colorLabels } = useCalendarStore();

  const isEdit = event !== null;
  const initialState = useMemo<FormState>(
    () => buildInitialState(event, defaultCalendarId, defaultDate, calendars),
    [event, defaultCalendarId, defaultDate, calendars],
  );

  const [form, setForm] = useState<FormState>(initialState);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // When editing a single occurrence of a recurring series the user
  // can apply changes to just this occurrence (creates an EXDATE +
  // standalone override) or to the whole series.
  const isOccurrence = isEdit && !!event && isOccurrenceId(event.id);
  const [editScope, setEditScope] = useState<EditScope>('occurrence');

  // Reset the form whenever the dialog is opened with new context. We
  // key on isOpen + initialState; isOpen=false keeps the previous form
  // around briefly while the close animation runs (if we ever add one).
  useEffect(() => {
    if (isOpen) {
      setForm(initialState);
      setError(null);
      setEditScope('occurrence');
    }
  }, [isOpen, initialState]);

  const update = useCallback(
    <K extends keyof FormState>(key: K, value: FormState[K]) => {
      setForm((prev) => ({ ...prev, [key]: value }));
    },
    [],
  );

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
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
      const end = toIso(form.endDate, form.endTime, form.allDay);
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
        if (isEdit && event) {
          const seriesId = event.id.includes('@')
            ? event.id.split('@')[0]
            : event.id;

          if (isOccurrence && editScope === 'occurrence' && event.recurrence) {
            // Single-instance override: add the original date to the
            // series EXDATE list, then create a standalone event with
            // the user's modified fields.
            const occIso = occurrenceIsoFromId(event.id);
            if (occIso) {
              await addEventExdate(seriesId, occIso);
              await apiCreateEvent({
                calendar_id: form.calendarId,
                title: trimmedTitle,
                description: form.description.trim() || null,
                location: form.location.trim() || null,
                start,
                end,
                all_day: form.allDay,
                recurrence: null,
                color_label: form.colorLabel,
                reminders: [],
                sound: null,
                attendees: [],
              });
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
          };
          await apiUpdateEvent(updated);
          announce(t('dialogs.event.updated', { title: trimmedTitle }));
        } else {
          await apiCreateEvent({
            calendar_id: form.calendarId,
            title: trimmedTitle,
            description: form.description.trim() || null,
            location: form.location.trim() || null,
            start,
            end,
            all_day: form.allDay,
            recurrence,
            color_label: form.colorLabel,
            reminders: [],
            sound: null,
            attendees: [],
          });
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
    [form, isEdit, event, isOccurrence, editScope, announce, onClose, t],
  );

  const onDelete = useCallback(async () => {
    if (!event) return;
    setError(null);
    setSubmitting(true);
    try {
      const seriesId = event.id.includes('@')
        ? event.id.split('@')[0]
        : event.id;
      if (isOccurrence && editScope === 'occurrence' && event.recurrence) {
        const occIso = occurrenceIsoFromId(event.id);
        if (occIso) {
          await addEventExdate(seriesId, occIso);
          announce(t('dialogs.event.occurrenceDeleted', { title: event.title }));
          onClose();
          return;
        }
      }
      await deleteEventById(seriesId);
      announce(t('dialogs.event.deleted', { title: event.title }));
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
  }, [event, isOccurrence, editScope, announce, onClose, t]);

  const title = isEdit ? t('dialogs.event.editTitle') : t('dialogs.event.newTitle');

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      className="modal--form"
      dismissOnBackdrop={false}
    >
      <form onSubmit={onSubmit} className="form">
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
            {calendars.map((cal) => (
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

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.event.fields.colorLabel')}
          </span>
          <select
            value={form.colorLabel ?? ''}
            onChange={(e) =>
              update('colorLabel', e.target.value ? e.target.value : null)
            }
          >
            <option value="">{t('dialogs.event.noColorLabel')}</option>
            {colorLabels.map((label) => (
              <option key={label.id} value={label.id}>
                {label.name}
              </option>
            ))}
          </select>
        </label>

        <RecurrenceSelector
          value={form.rrule}
          onChange={(rrule) => update('rrule', rrule)}
        />

        {isOccurrence && (
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
              disabled={submitting}
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
  );
}

function buildInitialState(
  event: CalendarEvent | null,
  defaultCalendarId: string | undefined,
  defaultDate: string | undefined,
  calendars: { id: string }[],
): FormState {
  if (event) {
    const start = new Date(event.start);
    const end = new Date(event.end);
    return {
      title: event.title,
      calendarId: event.calendar_id,
      startDate: dateInput(start),
      startTime: timeInput(start),
      endDate: dateInput(end),
      endTime: timeInput(end),
      allDay: event.all_day,
      location: event.location ?? '',
      description: event.description ?? '',
      rrule: event.recurrence?.rrule ?? null,
      colorLabel: event.color_label ?? null,
    };
  }

  // New event: 1-hour slot.
  //  - When the caller anchored us on a specific day (Enter on a week
  //    cell), we use 09:00–10:00 on that day so the form reflects the
  //    day the user is looking at rather than "today" o'clock.
  //  - Otherwise (toolbar / Ctrl+N) we default to the next full hour
  //    from now, which lines up with the user's day-of-work rhythm.
  const anchoredDay = parseDefaultDate(defaultDate);
  let start: Date;
  if (anchoredDay) {
    start = new Date(anchoredDay);
    start.setHours(9, 0, 0, 0);
  } else {
    start = new Date();
    start.setMinutes(0, 0, 0);
    start.setHours(start.getHours() + 1);
  }
  const end = new Date(start);
  end.setHours(end.getHours() + 1);

  const fallbackCalendar = defaultCalendarId ?? calendars[0]?.id ?? '';

  return {
    title: '',
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
  };
}

/** Parse a YYYY-MM-DD or full ISO string into a Date at the start of
 *  the local day. Returns null when the input is undefined / invalid. */
function parseDefaultDate(input: string | undefined): Date | null {
  if (!input) return null;
  // Accept both "YYYY-MM-DD" and full ISO. We strip the time so the
  // base sits at midnight local time; the caller's "next full hour"
  // logic then lands on 01:00 of that day, which is reasonable for a
  // form the user will customise anyway.
  const isoDay = input.length >= 10 ? input.slice(0, 10) : input;
  const [y, m, d] = isoDay.split('-').map(Number);
  if (!y || !m || !d) return null;
  const date = new Date(y, m - 1, d, 0, 0, 0);
  return Number.isNaN(date.getTime()) ? null : date;
}

function dateInput(d: Date): string {
  // `<input type="date">` expects ISO 8601 YYYY-MM-DD, always in local
  // time. Build it from the local components rather than `toISOString()`
  // which uses UTC and can shift the day on timezones east of GMT.
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

function timeInput(d: Date): string {
  const h = String(d.getHours()).padStart(2, '0');
  const m = String(d.getMinutes()).padStart(2, '0');
  return `${h}:${m}`;
}

function toIso(
  date: string,
  time: string,
  allDay: boolean,
): string | null {
  if (!date) return null;
  const [y, m, d] = date.split('-').map(Number);
  if (!y || !m || !d) return null;
  let hours = 0;
  let minutes = 0;
  if (!allDay) {
    const parts = time.split(':').map(Number);
    if (parts.length < 2 || parts.some(Number.isNaN)) return null;
    [hours, minutes] = parts;
  }
  return new Date(y, m - 1, d, hours, minutes, 0).toISOString();
}
