import { describe, expect, it, vi } from 'vitest';

/**
 * The phone's event-editor parameters for a reminder
 * (`mobile/src/reminders/tapParams.ts`): its notification and a row of the
 * reminders overview both come here, so the overview opens THAT occurrence of
 * a series too, not the series (TODO A5). The phone has no test runner of its
 * own, so its module runs here.
 */

/** Through a path the type checker does not follow (see
 *  `moveActions.parity.test.ts`). */
const PHONE_TAP_PARAMS = '../../mobile/src/reminders/tapParams';

interface Reads {
  listCalendars: () => Promise<{ id: string; invitations_reply_only?: boolean }[]>;
  getEventById: (id: string, calendarId: string) => Promise<unknown>;
}
async function params(
  tap: Record<string, string>,
  reads: Reads,
): Promise<Record<string, unknown> | null> {
  const phone = (await import(/* @vite-ignore */ PHONE_TAP_PARAMS)) as {
    eventEditorParams: (tap: unknown, reads: Reads) => Promise<Record<string, unknown> | null>;
  };
  return phone.eventEditorParams({ kind: 'event', ...tap }, reads);
}

const AT = '2026-11-16T09:00:00.000Z';
const TAP = { eventId: 'series-1', calendarId: 'work', start: AT, title: 'Standup' };
const SERIES = {
  id: 'series-1',
  calendar_id: 'work',
  start: '2026-11-02T09:00:00.000Z',
  end: '2026-11-02T09:15:00.000Z',
  all_day: false,
  recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [] as string[] },
};

describe('the phone opens a reminder', () => {
  it('as THAT occurrence of a series, the calendars listed before the row is read', async () => {
    const order: string[] = [];
    const result = await params(TAP, {
      listCalendars: async () => {
        order.push('calendars');
        return [{ id: 'work' }];
      },
      getEventById: async (id, calendarId) => {
        order.push(`row ${id} ${calendarId}`);
        return SERIES;
      },
    });
    expect(order).toEqual(['calendars', 'row series-1 work']);
    expect(result).toEqual({ eventId: 'series-1', calendarId: 'work', occurrence: AT });
  });

  it('a birthday by its title, without a read it cannot answer', async () => {
    const getEventById = vi.fn(async () => null);
    const result = await params(
      { ...TAP, eventId: 'aperio-birthday:contact-7:2026', calendarId: 'aperio-birthdays:l', title: 'Kim' },
      { listCalendars: async () => [], getEventById },
    );
    expect(getEventById).not.toHaveBeenCalled();
    expect(result?.initialTitle).toBe('Kim');
  });

  it('nothing, for an occurrence deleted since the reminder was planned', async () => {
    const result = await params(TAP, {
      listCalendars: async () => [{ id: 'work' }],
      getEventById: async () => ({ ...SERIES, recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [AT] } }),
    });
    expect(result).toBeNull();
  });

  it('as it is when nothing can be read', async () => {
    const result = await params(TAP, {
      listCalendars: () => Promise.reject(new Error('offline')),
      getEventById: () => Promise.reject(new Error('offline')),
    });
    expect(result).toEqual({ eventId: 'series-1', calendarId: 'work' });
  });
});
