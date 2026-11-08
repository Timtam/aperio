import { describe, expect, it, vi } from 'vitest';
import type { ReminderTap } from '@aperio/shared';

import type { CalendarEvent, Task } from '../api/types';
import { resolveReminderTap, type TapReads } from './reminderTapTarget';

/**
 * What a reminder opens on the desktop (TODO A5): the toast's click and Enter
 * in the reminders overview both resolve here, by the phone's rule.
 */

const AT = '2026-11-16T09:00:00.000Z';

const series = {
  id: 'series-1',
  calendar_id: 'work',
  title: 'Standup',
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T09:15:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [] },
} as unknown as CalendarEvent;

const tapOf = (eventId: string, calendarId = 'work', start = AT): ReminderTap => ({
  kind: 'event',
  eventId,
  calendarId,
  start,
  title: 'Standup',
});

function reads(over: Partial<TapReads> = {}): TapReads {
  return {
    calendars: [{ id: 'work' }],
    getEventById: vi.fn(() => Promise.resolve(null)),
    getEvents: vi.fn(() => Promise.resolve([])),
    getTaskById: vi.fn(() => Promise.resolve(null)),
    getTasks: vi.fn(() => Promise.resolve([])),
    ...over,
  };
}

describe('resolveReminderTap — events', () => {
  it('opens THAT occurrence of an own series, the scope left to the editor', async () => {
    const r = reads({ getEventById: vi.fn(() => Promise.resolve(series)) });
    const target = await resolveReminderTap(tapOf('series-1'), r);
    expect(r.getEventById).toHaveBeenCalledWith('series-1', 'work');
    expect(target.kind).toBe('event');
    if (target.kind !== 'event') return;
    expect(target.event.id).toBe(`series-1@${AT}`);
    expect(target.event.start).toBe(AT);
    expect(target.event.end).toBe('2026-11-16T09:15:00.000Z');
    expect(target.initialScope).toBeUndefined();
    expect(target.shownOccurrence).toBeUndefined();
  });

  it('says not found for an occurrence deleted since the reminder was planned', async () => {
    // Opened, and saved as "this occurrence", it would come back.
    const deleted = {
      ...series,
      recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [AT] },
    } as unknown as CalendarEvent;
    const r = reads({ getEventById: vi.fn(() => Promise.resolve(deleted)) });
    expect(await resolveReminderTap(tapOf('series-1'), r)).toEqual({ kind: 'notFound' });
  });

  it('says not found for an occurrence the series no longer reaches', async () => {
    const ended = {
      ...series,
      recurrence: { rrule: 'FREQ=WEEKLY;UNTIL=20261110T000000Z', exceptions: [] },
    } as unknown as CalendarEvent;
    const r = reads({ getEventById: vi.fn(() => Promise.resolve(ended)) });
    expect(await resolveReminderTap(tapOf('series-1'), r)).toEqual({ kind: 'notFound' });
  });

  it('opens a single change by its own id, as that occurrence', async () => {
    const override = {
      ...series,
      id: `series-1::rid::${AT}`,
      start: '2026-11-16T10:30:00.000Z',
      end: '2026-11-16T10:45:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    const r = reads({ getEventById: vi.fn(() => Promise.resolve(override)) });
    const target = await resolveReminderTap(tapOf(override.id, 'work', override.start), r);
    expect(target).toEqual({ kind: 'event', event: override, initialScope: 'occurrence' });
  });

  it("opens someone else's meeting as the series, showing the occurrence", async () => {
    const meeting = { ...series, calendar_id: 'icloud', organized_elsewhere: true } as CalendarEvent;
    const r = reads({
      calendars: [{ id: 'icloud', supports_scheduling: true, invitations_reply_only: true }],
      getEventById: vi.fn(() => Promise.resolve(meeting)),
    });
    const target = await resolveReminderTap(tapOf('series-1', 'icloud'), r);
    expect(target).toEqual({
      kind: 'event',
      event: meeting,
      initialScope: 'series',
      shownOccurrence: AT,
    });
  });

  it('opens a plain appointment as it is', async () => {
    const single = { ...series, id: 'one', recurrence: null } as unknown as CalendarEvent;
    const r = reads({ getEventById: vi.fn(() => Promise.resolve(single)) });
    expect(await resolveReminderTap(tapOf('one'), r)).toEqual({ kind: 'event', event: single });
  });

  it('finds a birthday in its calendar, where it is made up, not by id', async () => {
    const birthday = {
      id: 'aperio-birthday:contact-7:2026',
      calendar_id: 'aperio-birthdays:list-1',
      title: 'Kim (30)',
      all_day: true,
    } as unknown as CalendarEvent;
    const other = { ...birthday, id: 'aperio-birthday:contact-8:2026' } as CalendarEvent;
    const r = reads({ getEvents: vi.fn(() => Promise.resolve([other, birthday])) });
    const target = await resolveReminderTap(
      tapOf(birthday.id, birthday.calendar_id, '2026-11-20T23:00:00+00:00'),
      r,
    );
    expect(target).toEqual({ kind: 'event', event: birthday });
    expect(r.getEventById).not.toHaveBeenCalled();
    const [request] = (r.getEvents as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(request.calendar_id).toBe(birthday.calendar_id);
    expect(Date.parse(request.start)).toBeLessThan(Date.parse('2026-11-20T23:00:00Z'));
    expect(Date.parse(request.end)).toBeGreaterThan(Date.parse('2026-11-21T23:00:00Z'));
  });

  it('says not found, never a create editor, when the row is gone or unreadable', async () => {
    expect(await resolveReminderTap(tapOf('gone'), reads())).toEqual({ kind: 'notFound' });
    const failing = reads({ getEventById: vi.fn(() => Promise.reject(new Error('offline'))) });
    expect(await resolveReminderTap(tapOf('gone'), failing)).toEqual({ kind: 'notFound' });
    expect(
      await resolveReminderTap(tapOf('aperio-birthday:contact-9:2026', 'aperio-birthdays:l'), reads()),
    ).toEqual({ kind: 'notFound' });
  });
});

describe('resolveReminderTap — tasks and the day start', () => {
  const task = { id: 't-1', list_id: 'vikunja-3', title: 'Report' } as unknown as Task;
  const taskTap: ReminderTap = { kind: 'task', taskId: 't-1', listId: 'vikunja-3' };

  it('opens a stored task', async () => {
    const r = reads({ getTaskById: vi.fn(() => Promise.resolve(task)) });
    expect(await resolveReminderTap(taskTap, r)).toEqual({ kind: 'task', task });
    expect(r.getTasks).not.toHaveBeenCalled();
  });

  it('finds a task of an external list in its list', async () => {
    const r = reads({ getTasks: vi.fn(() => Promise.resolve([task])) });
    expect(await resolveReminderTap(taskTap, r)).toEqual({ kind: 'task', task });
    expect(r.getTasks).toHaveBeenCalledWith('vikunja-3');
  });

  it('says not found for a task that is gone', async () => {
    expect(await resolveReminderTap(taskTap, reads())).toEqual({ kind: 'notFound' });
  });

  it('opens the day-start review for its notification', async () => {
    expect(await resolveReminderTap({ kind: 'dayStart' }, reads())).toEqual({ kind: 'dayStart' });
  });
});
