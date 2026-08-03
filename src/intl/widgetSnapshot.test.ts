import { describe, expect, it } from 'vitest';

import {
  WIDGET_SNAPSHOT_VERSION,
  buildWidgetSnapshot,
  type RecurringEventLike,
} from '@aperio/shared';

import type { Task } from '../api/types';

/** A minimal event the builder can consume, plus the two fields the accessors
 *  pull out (the frontends spell them differently, hence the accessors). */
interface TestEvent extends RecurringEventLike {
  calendar_id: string;
  title: string;
  all_day: boolean;
}

const baseEvent: TestEvent = {
  id: 'e1',
  calendar_id: 'cal',
  title: 'Standup',
  start: '2026-08-03T08:00:00Z',
  end: '2026-08-03T08:30:00Z',
  all_day: false,
  recurrence: null,
};

const baseTask: Task = {
  id: 't1',
  list_id: 'list',
  title: 'thing',
  description: null,
  status: 'open',
  priority: 'medium',
  effort: 'medium',
  scheduled_date: null,
  scheduled_time: null,
  deadline_date: null,
  deadline_time: null,
  deadline_reminder_days: null,
  recurrence: null,
  resurface_date: null,
  series_id: null,
  parent_id: null,
  section_id: null,
  color_label: null,
  reminders: [],
  assignees: [],
  sound: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  completed_at: null,
  etag: null,
};

/** The builder is timezone-sensitive by design (day keys are LOCAL), so the
 *  fixtures below anchor "now" to a local wall-clock moment rather than a UTC
 *  instant — the same way the app's day maths does. */
function localAt(y: number, m: number, d: number, h = 0, min = 0): Date {
  return new Date(y, m - 1, d, h, min, 0, 0);
}

function build(
  patch: {
    events?: TestEvent[];
    tasks?: Task[];
    now?: Date;
    horizonDays?: number;
    limit?: number;
    hiddenContainers?: ReadonlySet<string>;
    eventColorOf?: (event: TestEvent) => string | null;
  } = {},
) {
  return buildWidgetSnapshot<TestEvent>({
    events: patch.events ?? [],
    tasks: patch.tasks ?? [],
    now: patch.now ?? localAt(2026, 8, 3, 7, 0),
    horizonDays: patch.horizonDays ?? 7,
    limit: patch.limit ?? 20,
    strings: {
      empty: 'Nichts geplant.',
      noTimed: 'Nichts mit Uhrzeit.',
      stale: 'Keine aktuellen Daten.',
      allDay: 'Ganztägig',
      today: 'Heute',
      runningUntil: 'Läuft bis {time}',
      kindEvent: 'Termin',
      kindTask: 'Aufgabe',
    },
    hiddenContainers: patch.hiddenContainers,
    eventColorOf: patch.eventColorOf,
    calendarIdOf: (e) => e.calendar_id,
    titleOf: (e) => e.title,
    allDayOf: (e) => e.all_day,
  });
}

describe('buildWidgetSnapshot — envelope', () => {
  it('stamps the version, the moment and the end of the covered window', () => {
    const now = localAt(2026, 8, 3, 7, 0);
    const snap = build({ now, horizonDays: 7 });
    expect(snap.version).toBe(WIDGET_SNAPSHOT_VERSION);
    // The words the widget cannot translate itself travel with the data.
    expect(snap.strings.empty).toBe('Nichts geplant.');
    expect(snap.generatedAt).toBe(now.toISOString());
    // The horizon is what tells the widget "no more data" apart from "nothing
    // planned" — an empty list alone cannot distinguish those.
    expect(new Date(snap.horizonEnd).getTime() - now.getTime()).toBe(7 * 86_400_000);
  });
});

describe('buildWidgetSnapshot — events', () => {
  it('keeps an event that is running right now', () => {
    const now = localAt(2026, 8, 3, 10, 15);
    const ev: TestEvent = {
      ...baseEvent,
      start: localAt(2026, 8, 3, 10, 0).toISOString(),
      end: localAt(2026, 8, 3, 11, 0).toISOString(),
    };
    // The one moment the user actually looks at the widget is while the meeting
    // is on; dropping it at its start time would be exactly wrong.
    expect(build({ events: [ev], now }).items.map((i) => i.id)).toEqual(['e1']);
  });

  it('drops an event that has already ended', () => {
    const now = localAt(2026, 8, 3, 12, 0);
    const ev: TestEvent = {
      ...baseEvent,
      start: localAt(2026, 8, 3, 10, 0).toISOString(),
      end: localAt(2026, 8, 3, 11, 0).toISOString(),
    };
    expect(build({ events: [ev], now }).items).toEqual([]);
  });

  it('keeps an all-day event for the whole of its day and drops it at midnight', () => {
    const allDay: TestEvent = {
      ...baseEvent,
      all_day: true,
      start: localAt(2026, 8, 3).toISOString(),
      // All-day ends are EXCLUSIVE — the next midnight.
      end: localAt(2026, 8, 4).toISOString(),
    };
    expect(build({ events: [allDay], now: localAt(2026, 8, 3, 23, 59) }).items).toHaveLength(1);
    expect(build({ events: [allDay], now: localAt(2026, 8, 4, 0, 1) }).items).toEqual([]);
  });

  it('expands a recurring series into one row per occurrence', () => {
    const daily: TestEvent = {
      ...baseEvent,
      start: localAt(2026, 8, 3, 9, 0).toISOString(),
      end: localAt(2026, 8, 3, 9, 30).toISOString(),
      recurrence: { rrule: 'FREQ=DAILY', exceptions: [] },
    };
    const snap = build({ events: [daily], now: localAt(2026, 8, 3, 7, 0), horizonDays: 3 });
    // A master alone would leave the widget showing one meeting and then
    // nothing for the rest of the week.
    expect(snap.items.length).toBeGreaterThan(1);
    expect(new Set(snap.items.map((i) => i.id)).size).toBe(snap.items.length);
  });

  it('leaves out a hidden calendar', () => {
    const snap = build({
      events: [{ ...baseEvent, start: localAt(2026, 8, 3, 9, 0).toISOString(), end: localAt(2026, 8, 3, 9, 30).toISOString() }],
      now: localAt(2026, 8, 3, 7, 0),
      hiddenContainers: new Set(['cal']),
    });
    expect(snap.items).toEqual([]);
  });

  it('carries the resolved container colour when there is one', () => {
    const snap = build({
      events: [{ ...baseEvent, start: localAt(2026, 8, 3, 9, 0).toISOString(), end: localAt(2026, 8, 3, 9, 30).toISOString() }],
      now: localAt(2026, 8, 3, 7, 0),
      eventColorOf: (e) => (e.calendar_id === 'cal' ? '#3b82f6' : null),
    });
    expect(snap.items[0]?.color).toBe('#3b82f6');
    // Absent rather than null — the widget's decoder treats the field as
    // optional, and a null would cost bytes to say nothing.
    expect(build({ events: [baseEvent], now: localAt(2026, 8, 3, 7, 0) }).items[0]?.color).toBeUndefined();
  });
});

describe('buildWidgetSnapshot — tasks', () => {
  it('places a timed task at its time and an untimed one at the start of its day', () => {
    const timed: Task = {
      ...baseTask,
      id: 'timed',
      scheduled_date: '2026-08-03',
      scheduled_time: '14:30:00',
    };
    const untimed: Task = { ...baseTask, id: 'untimed', scheduled_date: '2026-08-03' };
    const snap = build({ tasks: [timed, untimed], now: localAt(2026, 8, 3, 7, 0) });
    const byId = new Map(snap.items.map((i) => [i.id, i]));
    expect(byId.get('timed')?.at).toBe(localAt(2026, 8, 3, 14, 30).toISOString());
    expect(byId.get('timed')?.untimed).toBe(false);
    expect(byId.get('untimed')?.at).toBe(localAt(2026, 8, 3).toISOString());
    expect(byId.get('untimed')?.untimed).toBe(true);
  });

  it('keeps an untimed task for today even in the evening, but drops a passed time', () => {
    const timed: Task = {
      ...baseTask,
      id: 'timed',
      scheduled_date: '2026-08-03',
      scheduled_time: '09:00:00',
    };
    const untimed: Task = { ...baseTask, id: 'untimed', scheduled_date: '2026-08-03' };
    const snap = build({ tasks: [timed, untimed], now: localAt(2026, 8, 3, 20, 0) });
    // There is no moment at which an undated-but-today task stopped being
    // today's business, so it stands; a task pinned to 09:00 does not.
    expect(snap.items.map((i) => i.id)).toEqual(['untimed']);
  });

  it('leaves out completed, cancelled and hidden-list tasks', () => {
    const done: Task = {
      ...baseTask,
      id: 'done',
      status: 'completed',
      scheduled_date: '2026-08-04',
    };
    const cancelled: Task = {
      ...baseTask,
      id: 'cancelled',
      status: 'cancelled',
      scheduled_date: '2026-08-04',
    };
    const other: Task = {
      ...baseTask,
      id: 'other',
      list_id: 'hidden-list',
      scheduled_date: '2026-08-04',
    };
    const snap = build({
      tasks: [done, cancelled, other],
      now: localAt(2026, 8, 3, 7, 0),
      hiddenContainers: new Set(['hidden-list']),
    });
    expect(snap.items).toEqual([]);
  });

  it('does not reach past the horizon', () => {
    const far: Task = { ...baseTask, id: 'far', scheduled_date: '2026-08-20' };
    const near: Task = { ...baseTask, id: 'near', scheduled_date: '2026-08-05' };
    const snap = build({ tasks: [far, near], now: localAt(2026, 8, 3, 7, 0), horizonDays: 7 });
    expect(snap.items.map((i) => i.id)).toEqual(['near']);
  });
});

describe('buildWidgetSnapshot — ordering and cap', () => {
  it('sorts chronologically, events before tasks at the same instant', () => {
    const ev: TestEvent = {
      ...baseEvent,
      id: 'ev',
      start: localAt(2026, 8, 4, 9, 0).toISOString(),
      end: localAt(2026, 8, 4, 10, 0).toISOString(),
    };
    const sameTime: Task = {
      ...baseTask,
      id: 'task-at-9',
      scheduled_date: '2026-08-04',
      scheduled_time: '09:00:00',
    };
    const earlier: Task = {
      ...baseTask,
      id: 'task-at-8',
      scheduled_date: '2026-08-04',
      scheduled_time: '08:00:00',
    };
    const snap = build({
      events: [ev],
      tasks: [sameTime, earlier],
      now: localAt(2026, 8, 3, 7, 0),
    });
    expect(snap.items.map((i) => i.id)).toEqual(['task-at-8', 'ev', 'task-at-9']);
  });

  it('caps the list at the limit, keeping the soonest', () => {
    const tasks: Task[] = ['2026-08-04', '2026-08-05', '2026-08-06'].map((d, i) => ({
      ...baseTask,
      id: `t${i}`,
      scheduled_date: d,
    }));
    const snap = build({ tasks, now: localAt(2026, 8, 3, 7, 0), limit: 2 });
    expect(snap.items.map((i) => i.id)).toEqual(['t0', 't1']);
  });
});
