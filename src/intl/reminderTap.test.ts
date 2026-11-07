import { describe, expect, it } from 'vitest';

import {
  DAY_START_TAP_DATA,
  eventTapParams,
  reminderTapData,
  reminderTapOf,
  reminderTapOfUrl,
} from '../../shared/reminderTap';

const EVENT = {
  item_id: 'ev-1',
  item_kind: 'event' as const,
  container_id: 'work',
  start: '2026-11-09T09:00:00+00:00',
  title: 'Standup',
};

describe('reminderTapOf — what the scheduler wrote is what a tap reads', () => {
  it('reads an event reminder back', () => {
    expect(reminderTapOf(reminderTapData(EVENT))).toEqual({
      kind: 'event',
      eventId: 'ev-1',
      calendarId: 'work',
      start: EVENT.start,
      title: 'Standup',
    });
  });

  it('reads a task reminder back, with its list', () => {
    const data = reminderTapData({ ...EVENT, item_id: 't-1', item_kind: 'task', container_id: 'inbox' });
    expect(reminderTapOf(data)).toEqual({ kind: 'task', taskId: 't-1', listId: 'inbox' });
  });

  it('reads the day-start notification', () => {
    expect(reminderTapOf(DAY_START_TAP_DATA)).toEqual({ kind: 'dayStart' });
  });

  it('opens nothing for a notification of an older build, or of someone else', () => {
    // Before this change a reminder carried no calendar.
    expect(reminderTapOf({ itemId: 'ev-1', itemKind: 'event' })).toBeNull();
    expect(reminderTapOf({ ...reminderTapData(EVENT), start: 'soon' })).toBeNull();
    expect(reminderTapOf({ ...reminderTapData(EVENT), itemKind: 'note' })).toBeNull();
    expect(reminderTapOf(null)).toBeNull();
    expect(reminderTapOf('x')).toBeNull();
  });
});

describe('eventTapParams — a tap opens the occurrence, without asking', () => {
  const tap = {
    kind: 'event' as const,
    eventId: 'ev-1',
    calendarId: 'work',
    start: '2026-11-09T09:00:00+00:00',
    title: 'Standup',
  };
  const series = { recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [] } };

  it('opens a single event as itself', () => {
    expect(eventTapParams(tap, { recurrence: null }, null)).toEqual({
      eventId: 'ev-1',
      calendarId: 'work',
    });
  });

  it('opens an occurrence of a series at that occurrence, the scope left to the editor', () => {
    // In the spelling the calendar's own expansion writes.
    expect(eventTapParams(tap, series, null)).toEqual({
      eventId: 'ev-1',
      calendarId: 'work',
      occurrence: '2026-11-09T09:00:00.000Z',
    });
  });

  it('opens a single change of a series by its own id, at the slot it stands in for', () => {
    const slot = '2026-11-09T09:00:00Z';
    const moved = { ...tap, eventId: `ev-1::rid::${slot}`, start: '2026-11-09T11:00:00+00:00' };
    expect(eventTapParams(moved, { recurrence: null }, null)).toEqual({
      eventId: `ev-1::rid::${slot}`,
      calendarId: 'work',
      occurrence: slot,
      initialScope: 'occurrence',
    });
  });

  it("opens someone else's meeting as the series, showing the occurrence's times", () => {
    const locked = { invitations_reply_only: true };
    expect(eventTapParams(tap, { ...series, organized_elsewhere: true }, locked)).toEqual({
      eventId: 'ev-1',
      calendarId: 'work',
      initialScope: 'series',
      shownOccurrence: '2026-11-09T09:00:00.000Z',
    });
  });

  it('opens a birthday with its name, which it has no row to read from', () => {
    const birthday = { ...tap, eventId: 'aperio-birthday:c1:2026', title: 'Kim' };
    expect(eventTapParams(birthday, null, null)).toEqual({
      eventId: 'aperio-birthday:c1:2026',
      calendarId: 'work',
      initialTitle: 'Kim',
    });
  });

  it('opens the event as it is when its row could not be read', () => {
    expect(eventTapParams(tap, null, null)).toEqual({ eventId: 'ev-1', calendarId: 'work' });
  });
});

describe('reminderTapOfUrl — a tapped Live Activity', () => {
  // As the activity writes it: every value encoded down to letters and digits.
  const enc = (s: string) =>
    Array.from(new TextEncoder().encode(s))
      .map((b) =>
        /[A-Za-z0-9]/.test(String.fromCharCode(b)) ? String.fromCharCode(b) : `%${b.toString(16).toUpperCase().padStart(2, '0')}`,
      )
      .join('');
  const url = (fields: Record<string, string>) =>
    `aperio://event?${Object.entries(fields)
      .map(([k, v]) => `${k}=${enc(v)}`)
      .join('&')}`;

  it('reads the appointment back, with a title that holds & and = and an offset with +', () => {
    const tap = reminderTapOfUrl(
      url({ eventId: 'ev-1', calendarId: 'acc::work', start: '2026-11-09T10:00:00+01:00', title: 'Q&A = Fragen' }),
    );
    expect(tap).toEqual({
      kind: 'event',
      eventId: 'ev-1',
      calendarId: 'acc::work',
      start: '2026-11-09T10:00:00+01:00',
      title: 'Q&A = Fragen',
    });
  });

  it('ignores any other address', () => {
    expect(reminderTapOfUrl('file:///tmp/invite.ics')).toBeNull();
    expect(reminderTapOfUrl('aperio://other?eventId=1')).toBeNull();
    expect(reminderTapOfUrl('aperio://event?eventId=ev-1')).toBeNull();
  });
});
