import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  type EventGroup,
  memberFromEvent,
  eventGroupMemberKey,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  dissolveEventGroup,
  eventGroupsForEvents,
  groupEvents,
  isCommandError,
  ungroupEvent,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { useDateFormat } from '../intl/dateFormat';
import { seriesIdOf } from '../intl/recurrence';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useEvents } from '../state/useEvents';
import { Modal } from './Modal';

/**
 * "These events mean the same appointment" (`DESIGN-event-groups.md`).
 *
 * The same commitment routinely exists several times over — in the work
 * calendar so colleagues see it, copied into a private calendar because that
 * is the one a voice assistant reads out. This is where the user says so.
 *
 * ONE event at a time, deliberately. A multi-select spanning calendars is a
 * pointing gesture with no keyboard equivalent worth the name; naming a second
 * event from a list is the same statement, reachable by everyone. The list is
 * the OTHER events of that day, which is where a duplicate of an appointment
 * lives by definition.
 *
 * Nothing here touches a provider: grouping two events changes neither of
 * them, and ungrouping leaves both exactly as they were.
 */
export interface EventGroupDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** The event the user opened the menu on. */
  event: CalendarEvent;
  /** Re-read the views once the grouping changed. */
  onChanged?: () => void;
}

/** The day the anchor event starts, as a local midnight-to-midnight range. */
function dayRangeOf(event: CalendarEvent): { start: Date; end: Date } {
  const start = new Date(event.start);
  const from = new Date(
    start.getFullYear(),
    start.getMonth(),
    start.getDate(),
    0,
    0,
    0,
    0,
  );
  const to = new Date(from);
  to.setDate(to.getDate() + 1);
  return { start: from, end: to };
}

export function EventGroupDialog({
  isOpen,
  onClose,
  event,
  onChanged,
}: EventGroupDialogProps) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { calendars } = useCalendarStore();
  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
  );

  const anchorId = seriesIdOf(event);
  const anchorKey = eventGroupMemberKey(event.calendar_id, anchorId);
  const range = useMemo(() => dayRangeOf(event), [event]);
  const { events } = useEvents(range);

  // `undefined` while the lookup is in flight — distinct from `null`, which is
  // the answer "not grouped". Without the distinction the dialog would claim
  // the event is ungrouped for the first frame of every open.
  const [group, setGroup] = useState<EventGroup | null | undefined>(undefined);
  const [picked, setPicked] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const messageId = useId();

  const load = useCallback(async () => {
    try {
      const groups = await eventGroupsForEvents([
        { calendar_id: event.calendar_id, event_id: anchorId },
      ]);
      setGroup(groups[0] ?? null);
    } catch {
      // A failed lookup reads as "not grouped" rather than as a blank dialog:
      // the picker below still works, and grouping is idempotent.
      setGroup(null);
    }
  }, [event.calendar_id, anchorId]);

  useEffect(() => {
    if (!isOpen) return;
    setPicked('');
    setError(null);
    setGroup(undefined);
    void load();
  }, [isOpen, load]);

  // Focus Close on open: everything else here changes data.
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => closeRef.current?.focus());
  }, [isOpen]);

  /**
   * The events that can be named as "the same appointment".
   *
   * By SERIES, not by occurrence: a recurring event shows up once per day in
   * the range, and offering the same series three times would be three ways to
   * say one thing. Members of the anchor's own group drop out — they already
   * say it.
   */
  const candidates = useMemo(() => {
    const seen = new Set<string>([anchorKey]);
    for (const m of group?.members ?? []) {
      seen.add(eventGroupMemberKey(m.calendar_id, m.event_id));
    }
    const out: CalendarEvent[] = [];
    for (const ev of events) {
      const key = eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev));
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(ev);
    }
    return out;
  }, [events, group, anchorKey]);

  const describeEvent = useCallback(
    (ev: { title: string; start: string; all_day: boolean; calendar_id: string }) =>
      t('dialogs.eventGroup.candidate', {
        title: ev.title,
        time: ev.all_day
          ? t('dialogs.eventGroup.allDay')
          : fmt.format(new Date(ev.start), 'p'),
        calendar: calendarName(ev.calendar_id),
      }),
    [t, fmt, calendarName],
  );

  const fail = useCallback(
    (err: unknown) => {
      // The one refusal a user can actually meet: both events are already
      // grouped, with different partners. Only they can decide what that
      // should become, so it is said plainly instead of as a database error.
      const message =
        isCommandError(err) && err.code === 'event_group_conflict'
          ? t('dialogs.eventGroup.conflict')
          : isCommandError(err)
            ? err.message
            : String(err);
      setError(message);
      announce(message);
    },
    [t, announce],
  );

  const addPicked = async () => {
    if (busy || picked === '') return;
    const other = candidates.find(
      (ev) => eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)) === picked,
    );
    if (!other) return;
    setBusy(true);
    setError(null);
    try {
      const next = await groupEvents([
        memberFromEvent({ ...event, id: anchorId }),
        memberFromEvent({ ...other, id: seriesIdOf(other) }),
      ]);
      setGroup(next);
      setPicked('');
      announce(
        t('dialogs.eventGroup.grouped', {
          title: event.title,
          other: other.title,
        }),
      );
      onChanged?.();
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  };

  const removeSelf = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await ungroupEvent(event.calendar_id, anchorId);
      setGroup(next);
      announce(t('dialogs.eventGroup.ungrouped', { title: event.title }));
      onChanged?.();
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  };

  const dissolve = async () => {
    if (busy || !group) return;
    setBusy(true);
    setError(null);
    try {
      await dissolveEventGroup(group.id);
      setGroup(null);
      announce(t('dialogs.eventGroup.dissolved'));
      onChanged?.();
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  };

  const members = group?.members ?? [];

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.eventGroup.title')}
      describedById={messageId}
    >
      <p id={messageId} className="form__message">
        {group === undefined
          ? t('dialogs.eventGroup.loading')
          : members.length > 0
            ? t('dialogs.eventGroup.inGroup', {
                title: event.title,
                count: members.length - 1,
              })
            : t('dialogs.eventGroup.notGrouped', { title: event.title })}
      </p>

      {members.length > 0 && (
        <ul className="form__message">
          {members.map((m) => (
            <li key={eventGroupMemberKey(m.calendar_id, m.event_id)}>
              {t('dialogs.eventGroup.member', {
                title: m.title,
                calendar: calendarName(m.calendar_id),
              })}
            </li>
          ))}
        </ul>
      )}

      <label className="form__field">
        <span className="form__label">{t('dialogs.eventGroup.pickLabel')}</span>
        <select
          value={picked}
          onChange={(e) => setPicked(e.target.value)}
          disabled={candidates.length === 0}
        >
          <option value="">{t('dialogs.eventGroup.pickNone')}</option>
          {candidates.map((ev) => (
            <option
              key={eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev))}
              value={eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev))}
            >
              {describeEvent(ev)}
            </option>
          ))}
        </select>
        <span className="form__hint">
          {candidates.length === 0
            ? t('dialogs.eventGroup.noCandidates')
            : t('dialogs.eventGroup.pickHint')}
        </span>
      </label>

      {error && (
        <p className="form__error" role="alert">
          {error}
        </p>
      )}

      <div className="form__actions">
        <button ref={closeRef} type="button" onClick={onClose} className="form__action">
          {t('dialogs.eventGroup.close')}
        </button>
        <button
          type="button"
          onClick={() => void addPicked()}
          className="form__action form__action--primary"
          // aria-disabled, not `disabled`: a disabled button drops out of the
          // tab order, so a screen-reader user never hears that the action
          // exists or why it is unavailable. The handler guards instead.
          aria-disabled={busy || picked === '' || undefined}
        >
          {t('dialogs.eventGroup.add')}
        </button>
        {members.length > 0 && (
          <>
            <button
              type="button"
              onClick={() => void removeSelf()}
              className="form__action"
              aria-disabled={busy || undefined}
            >
              {t('dialogs.eventGroup.removeSelf')}
            </button>
            <button
              type="button"
              onClick={() => void dissolve()}
              className="form__action form__action--danger"
              aria-disabled={busy || undefined}
            >
              {t('dialogs.eventGroup.dissolve')}
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}
