import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  type EventGroup,
  memberFromEvent,
  eventGroupMemberKey,
  suggestGroupMate,
  withoutDuplicateMeetings,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  dissolveEventGroup,
  eventGroupsForEvents,
  getEvents,
  groupEvents,
  isCommandError,
  ungroupEvent,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { useDateFormat } from '../intl/dateFormat';
import { seriesIdOf } from '../intl/recurrence';
import { expandAll } from '../intl/recurrence';
import { useCalendarStore } from '../state/calendarStoreContext';
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
  const { calendars, selectedCalendarIds } = useCalendarStore();
  const calendarName = useCallback(
    (id: string) => calendars.find((c) => c.id === id)?.name ?? id,
    [calendars],
  );

  const anchorId = seriesIdOf(event);
  const anchorKey = eventGroupMemberKey(event.calendar_id, anchorId);
  const range = useMemo(() => dayRangeOf(event), [event]);

  /**
   * The day's events, fetched here rather than through `useEvents`.
   *
   * `useEvents` honours FOCUS MODE, which collapses the whole app to one
   * calendar — and a picker that can only offer events from the calendar the
   * anchor is already in is a picker for the one thing grouping is not for.
   * The copy this dialog exists to name lives in a DIFFERENT calendar by
   * definition.
   *
   * Selected calendars, then, and the anchor's own even when it is not (the
   * user just came from it). Expanded like every calendar surface: the
   * adapters hand back a recurring series as its master row, so unexpanded it
   * would offer series that do not occur on this day and hide the ones whose
   * master lies outside it.
   */
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    const ids = new Set([...selectedCalendarIds, event.calendar_id]);
    void (async () => {
      const perCalendar = await Promise.all(
        [...ids].map((id) =>
          getEvents({
            calendar_id: id,
            start: range.start.toISOString(),
            end: range.end.toISOString(),
          }).catch(() => [] as CalendarEvent[]),
        ),
      );
      if (cancelled) return;
      // `withoutDuplicateMeetings` for the same reason every view runs it: a
      // videoconference account contributes a read-only calendar of its own
      // meetings, and the ones that already have a calendar entry are dropped
      // there. Offering them here would invite the user to group an event with
      // a row that is not shown anywhere.
      setEvents(withoutDuplicateMeetings(expandAll(perCalendar.flat(), range)));
    })();
    return () => {
      cancelled = true;
    };
  }, [isOpen, range, selectedCalendarIds, event.calendar_id]);

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
   * Land focus back on Close after an action removes the control the user is
   * standing on.
   *
   * Taking the last-but-one member out dissolves the group, so "Take this
   * event out" and "Dissolve group" both UNMOUNT the moment they succeed —
   * with focus on them. Focus then falls to `<body>`, NVDA leaves application
   * mode, and Escape and Tab go dead inside a dialog that is still open. The
   * rAF runs after the removal commits and before the Modal's last-resort
   * recovery frame, so this informed repark wins.
   */
  const reparkFocus = useCallback(() => {
    requestAnimationFrame(() => closeRef.current?.focus({ preventScroll: true }));
  }, []);

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

  /**
   * What to call a member.
   *
   * The stored `title` is the SIGNATURE — what the event was called when it
   * joined, kept so a member whose provider id changed can be found again. It
   * is explicitly not for display: after a rename it is simply wrong. So the
   * day's loaded events answer first, and the signature is the fallback for a
   * member that is not in the loaded range (where a stale name still beats no
   * name at all).
   */
  const memberTitle = useCallback(
    (m: { calendar_id: string; event_id: string; title: string }) => {
      const key = eventGroupMemberKey(m.calendar_id, m.event_id);
      const live = events.find(
        (ev) => eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)) === key,
      );
      return live?.title ?? m.title;
    },
    [events],
  );

  /**
   * The candidate that looks like a copy of this event — same name, same
   * start, another calendar (`suggestGroupMate`).
   *
   * Offered, never applied: it arrives as the picker's preselection with a
   * line saying why, so confirming is one keystroke and disagreeing is just
   * picking something else. The design rejected grouping automatically for a
   * concrete reason — an office full of "Team meeting" at 10:00 — and a
   * preselection the user must still confirm keeps the recognition without
   * the wrong answer.
   */
  const suggested = useMemo(
    () => suggestGroupMate({ ...event, id: anchorId }, candidates),
    [event, anchorId, candidates],
  );
  // Applied ONCE per open. A ref rather than reading `picked` in the effect:
  // the point is "did we already offer this", which is not the same question
  // as "is something chosen right now" — a user who deliberately clears the
  // picker back to nothing must not have the suggestion pushed back at them.
  const suggestionOffered = useRef(false);
  useEffect(() => {
    if (!isOpen) suggestionOffered.current = false;
  }, [isOpen]);
  useEffect(() => {
    if (!isOpen || suggestionOffered.current || suggested == null) return;
    suggestionOffered.current = true;
    setPicked(eventGroupMemberKey(suggested.calendar_id, seriesIdOf(suggested)));
  }, [isOpen, suggested]);

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
      // Set only. The <p role="alert"> below speaks it the moment it appears —
      // announcing it as well made every failure arrive twice.
      setError(message);
    },
    [t],
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
      await ungroupEvent(event.calendar_id, anchorId);
      // `null`, not the group that came back: the call returns what is LEFT of
      // the group, which — with three or more members — still exists without
      // this event in it. Storing that made the dialog go on claiming this
      // event was grouped, and go on offering to dissolve a group it had just
      // left. What this screen states is always about THIS event.
      setGroup(null);
      announce(t('dialogs.eventGroup.ungrouped', { title: event.title }));
      // Both buttons unmount with the membership the user was standing on.
      reparkFocus();
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
      reparkFocus();
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
      {/* The dialog's whole answer — grouped or not, and with what — is prose,
          and prose is invisible to NVDA inside the Modal's role="application"
          body. `describedById` alone speaks it once at open and never again,
          so a state that CHANGES under the user (they just grouped something)
          would be stated only in the announcement. FocusableNote makes each
          line something the reading cursor can stop on, at any time. */}
      <FocusableNote id={messageId} className="form__message">
        {group === undefined
          ? t('dialogs.eventGroup.loading')
          : members.length > 0
            ? t('dialogs.eventGroup.inGroup', {
                title: event.title,
                count: members.length - 1,
              })
            : t('dialogs.eventGroup.notGrouped', { title: event.title })}
      </FocusableNote>

      {members.map((m) => (
        <FocusableNote
          key={eventGroupMemberKey(m.calendar_id, m.event_id)}
          className="form__message"
        >
          {t('dialogs.eventGroup.member', {
            title: memberTitle(m),
            calendar: calendarName(m.calendar_id),
          })}
        </FocusableNote>
      ))}

      <label className="form__field">
        <span className="form__label">{t('dialogs.eventGroup.pickLabel')}</span>
        {/* Never `disabled`: a disabled control leaves the tab order, taking
            the hint below it out of reach — and "there is no other event that
            day" is exactly what a user standing here needs to hear. With no
            candidates it holds its placeholder alone and Group refuses. */}
        <select value={picked} onChange={(e) => setPicked(e.target.value)}>
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
            : suggested
              ? t('dialogs.eventGroup.suggestHint', { title: suggested.title })
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
