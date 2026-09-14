import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent, Task } from '../api/types';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import {
  EVENT_DND_TYPE,
  SeriesShiftRefusedError,
  moveEventToDay,
  moveEventToSlot,
  moveOrCopyEvent,
  moveTaskToBacklog,
  readEventDrag,
  readTaskDrag,
  scheduleTaskOnDay,
  setEventDrag,
  setTaskDrag,
  TASK_DND_TYPE,
} from './moveActions';

/** An expanded recurring occurrence (what the views/dialog hand in). */
function occurrence(): CalendarEvent {
  return {
    id: 'e1@2026-06-15T09:00:00.000Z',
    series_id: 'e1',
    occurrence_start: '2026-06-15T09:00:00.000Z',
    calendar_id: 'c1',
    title: 'Standup',
    description: null,
    location: null,
    start: '2026-06-15T09:00:00.000Z',
    end: '2026-06-15T09:30:00.000Z',
    all_day: false,
    recurrence: { rrule: 'FREQ=DAILY', exceptions: [] },
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
  } as unknown as CalendarEvent;
}

/** Minimal DataTransfer stand-in (jsdom doesn't supply one). */
function fakeDataTransfer(): DataTransfer {
  const store = new Map<string, string>();
  return {
    effectAllowed: '',
    setData(type: string, val: string) {
      store.set(type, val);
    },
    getData(type: string) {
      return store.get(type) ?? '';
    },
    get types() {
      return Array.from(store.keys());
    },
  } as unknown as DataTransfer;
}

describe('moveActions drag payloads', () => {
  it('round-trips a task drag (incl. children + legacy id)', () => {
    const dt = fakeDataTransfer();
    const task = { id: 't1', list_id: 'L1', title: 'A' } as Task;
    const child = { id: 't2', parent_id: 't1' } as Task;
    setTaskDrag(dt, task, [child]);

    // The custom type is visible on dragover (via `types`), and the legacy
    // text/aperio-task id keeps the week-planner day-drop working.
    expect(dt.types).toContain(TASK_DND_TYPE);
    expect(dt.getData('text/aperio-task')).toBe('t1');

    const payload = readTaskDrag(dt);
    expect(payload?.task.id).toBe('t1');
    expect(payload?.children).toHaveLength(1);
    expect(payload?.children[0].id).toBe('t2');
  });

  it('returns null for a missing/invalid task payload', () => {
    expect(readTaskDrag(fakeDataTransfer())).toBeNull();
  });

  it('round-trips an event drag', () => {
    const dt = fakeDataTransfer();
    const event = { id: 'e1', calendar_id: 'c1', title: 'X' } as CalendarEvent;
    setEventDrag(dt, event);

    expect(dt.types).toContain(EVENT_DND_TYPE);
    const back = readEventDrag(dt);
    expect(back?.id).toBe('e1');
    expect(back?.calendar_id).toBe('c1');
  });

  it('returns null for a missing event payload', () => {
    expect(readEventDrag(fakeDataTransfer())).toBeNull();
  });
});

describe('moveActions time-axis moves', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(
      (_cmd: string, args: { task: Task }) => args.task,
    );
  });

  it('scheduleTaskOnDay sets scheduled_date on the day', async () => {
    await scheduleTaskOnDay({ id: 't', scheduled_date: null } as Task, '2026-06-15');
    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe('update_task');
    expect(args.task.scheduled_date).toBe('2026-06-15');
  });

  it('moveTaskToBacklog unschedules + reopens a completed task but KEEPS the deadline', async () => {
    await moveTaskToBacklog({
      id: 't',
      scheduled_date: '2026-06-15',
      scheduled_time: '09:00',
      deadline_date: '2026-06-20',
      deadline_time: '17:00',
      status: 'completed',
    } as Task);
    const { task } = invokeMock.mock.calls[0][1];
    expect(task.scheduled_date).toBeNull();
    expect(task.scheduled_time).toBeNull();
    // The deadline is independent data — unscheduling must not drop it.
    expect(task.deadline_date).toBe('2026-06-20');
    expect(task.deadline_time).toBe('17:00');
    expect(task.status).toBe('open');
  });

  it('moveTaskToBacklog preserves a non-completed status', async () => {
    await moveTaskToBacklog({
      id: 't',
      status: 'in_progress',
      scheduled_date: '2026-06-15',
    } as Task);
    expect(invokeMock.mock.calls[0][1].task.status).toBe('in_progress');
  });
});

describe('moveOrCopyEvent recurrence scope (§7.5)', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({});
  });

  it('copy + occurrence → one standalone create, recurrence stripped, no EXDATE', async () => {
    await moveOrCopyEvent(occurrence(), 'c2', 'copy', 'occurrence');
    expect(invokeMock.mock.calls).toHaveLength(1);
    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe('create_event');
    expect(args.request.calendar_id).toBe('c2');
    expect(args.request.recurrence).toBeNull();
    // The occurrence's concrete time, not the master's stored start.
    expect(args.request.start).toBe('2026-06-15T09:00:00.000Z');
  });

  it('move + occurrence → create standalone, THEN EXDATE the source series', async () => {
    await moveOrCopyEvent(occurrence(), 'c2', 'move', 'occurrence');
    const calls = invokeMock.mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0][0]).toBe('create_event');
    expect(calls[0][1].request.recurrence).toBeNull();
    // EXDATE targets the MASTER series id + the occurrence date on the source.
    expect(calls[1][0]).toBe('add_event_exdate');
    expect(calls[1][1]).toMatchObject({
      id: 'e1',
      occurrence: '2026-06-15T09:00:00.000Z',
      calendarId: 'c1',
    });
  });

  /** The series `occurrence()` belongs to: it starts two weeks earlier. */
  const seriesMaster = () =>
    ({
      ...occurrence(),
      id: 'e1',
      series_id: null,
      occurrence_start: null,
      start: '2026-06-01T09:00:00.000Z',
      end: '2026-06-01T09:30:00.000Z',
    }) as unknown as CalendarEvent;
  const serving = (series: CalendarEvent | null) =>
    invokeMock.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === 'get_event_by_id' ? series : {}),
    );
  const callsTo = (cmd: string) => invokeMock.mock.calls.filter((call) => call[0] === cmd);

  it('copy + series → copies the series from its own start with its rule, no EXDATE', async () => {
    serving(seriesMaster());
    await moveOrCopyEvent(occurrence(), 'c2', 'copy', 'series');
    const creates = callsTo('create_event');
    expect(creates).toHaveLength(1);
    expect(creates[0][1].request.recurrence).toMatchObject({ rrule: 'FREQ=DAILY' });
    // From an occurrence the copy used to start at that occurrence.
    expect(creates[0][1].request.start).toBe('2026-06-01T09:00:00.000Z');
    expect(callsTo('add_event_exdate')).toHaveLength(0);
  });

  it('move + series → moves the master as it is via update_event, no create/EXDATE', async () => {
    serving(seriesMaster());
    await moveOrCopyEvent(occurrence(), 'c2', 'move', 'series');
    const updates = callsTo('update_event');
    expect(updates).toHaveLength(1);
    const { event } = updates[0][1];
    expect(event.id).toBe('e1'); // master series id, not the occurrence id
    expect(event.calendar_id).toBe('c2');
    expect(event.start).toBe('2026-06-01T09:00:00.000Z');
    expect(callsTo('create_event')).toHaveLength(0);
  });

  it('move + series from a provider override keeps the rule of the master', async () => {
    // An override carries no rule; written as the series it dropped the rule.
    serving(seriesMaster());
    const override = {
      ...occurrence(),
      id: 'e1::rid::2026-06-15T09:00:00Z',
      series_id: null,
      occurrence_start: null,
      recurrence: null,
    } as unknown as CalendarEvent;
    await moveOrCopyEvent(override, 'c2', 'move', 'series');
    const { event } = callsTo('update_event')[0][1];
    expect(event.id).toBe('e1');
    expect(event.recurrence).toMatchObject({ rrule: 'FREQ=DAILY' });
    expect(event.start).toBe('2026-06-01T09:00:00.000Z');
  });

  it('writes nothing when the series cannot be loaded', async () => {
    serving(null);
    await expect(moveOrCopyEvent(occurrence(), 'c2', 'move', 'series')).rejects.toThrow(
      /could not be loaded/,
    );
    await expect(moveOrCopyEvent(occurrence(), 'c2', 'copy', 'series')).rejects.toThrow(
      /could not be loaded/,
    );
    expect(callsTo('update_event')).toHaveLength(0);
    expect(callsTo('create_event')).toHaveLength(0);
  });

  it('a non-recurring event ignores occurrence scope (whole-row move)', async () => {
    const plain = {
      id: 'p1',
      calendar_id: 'c1',
      title: 'One-off',
      start: '2026-06-15T09:00:00.000Z',
      end: '2026-06-15T10:00:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    await moveOrCopyEvent(plain, 'c2', 'move', 'occurrence');
    // Not an expanded occurrence → falls back to whole-series move (update).
    expect(invokeMock.mock.calls).toHaveLength(1);
    expect(invokeMock.mock.calls[0][0]).toBe('update_event');
  });
});

describe('moveEventToDay (planner drag-and-drop)', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({});
  });

  /** Local YYYY-MM-DD of an instant (mirrors the views' day keys). */
  const localKey = (iso: string, plusDays = 0) => {
    const d = new Date(iso);
    d.setDate(d.getDate() + plusDays);
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    return `${d.getFullYear()}-${mm}-${dd}`;
  };

  it('moves a plain event to another day, keeping time + duration', async () => {
    const plain = {
      id: 'p1',
      calendar_id: 'c1',
      title: 'One-off',
      start: '2026-06-15T09:00:00.000Z',
      end: '2026-06-15T10:30:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    const target = localKey(plain.start, 2);
    const moved = await moveEventToDay(plain, target);
    expect(moved).toBe(true);
    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe('update_event');
    const newStart = new Date(args.event.start);
    const newEnd = new Date(args.event.end);
    // Landed on the target local day…
    expect(localKey(args.event.start)).toBe(target);
    // …with the wall-clock time preserved…
    const old = new Date(plain.start);
    expect(newStart.getHours()).toBe(old.getHours());
    expect(newStart.getMinutes()).toBe(old.getMinutes());
    // …and the duration intact (90 min).
    expect(newEnd.getTime() - newStart.getTime()).toBe(90 * 60 * 1000);
  });

  it('same-day drop is a no-op', async () => {
    const plain = {
      id: 'p1',
      calendar_id: 'c1',
      start: '2026-06-15T09:00:00.000Z',
      end: '2026-06-15T10:00:00.000Z',
      recurrence: null,
    } as unknown as CalendarEvent;
    const moved = await moveEventToDay(plain, localKey(plain.start));
    expect(moved).toBe(false);
    expect(invokeMock.mock.calls).toHaveLength(0);
  });

  it('occurrence scope detaches: standalone create on the target day, then EXDATE', async () => {
    const occ = occurrence();
    const target = localKey(occ.start, 3);
    const moved = await moveEventToDay(occ, target, 'occurrence');
    expect(moved).toBe(true);
    const calls = invokeMock.mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0][0]).toBe('create_event');
    expect(calls[0][1].request.calendar_id).toBe('c1'); // same calendar
    expect(calls[0][1].request.recurrence).toBeNull(); // detached
    expect(localKey(calls[0][1].request.start)).toBe(target);
    expect(calls[1][0]).toBe('add_event_exdate');
    expect(calls[1][1]).toMatchObject({
      id: 'e1',
      occurrence: '2026-06-15T09:00:00.000Z',
      calendarId: 'c1',
    });
  });

});

describe('moving a whole series (dragged with the whole-series scope)', () => {
  /** A weekly Monday series with an end date and one excluded Monday. */
  const master = {
    id: 'e1',
    calendar_id: 'c1',
    title: 'Standup',
    start: '2026-06-15T07:00:00.000Z',
    end: '2026-06-15T07:30:00.000Z',
    all_day: false,
    recurrence: {
      rrule: 'FREQ=WEEKLY;BYDAY=MO;UNTIL=20260831T235959Z',
      exceptions: ['2026-06-29T07:00:00.000Z'],
    },
  } as unknown as CalendarEvent;

  const occurrenceOf = (series: CalendarEvent, iso: string) =>
    ({
      ...series,
      id: `e1@${iso}`,
      series_id: 'e1',
      occurrence_start: iso,
      start: iso,
      end: new Date(Date.parse(iso) + 30 * 60_000).toISOString(),
    }) as unknown as CalendarEvent;

  const localKey = (iso: string, plusDays = 0) => {
    const d = new Date(iso);
    d.setDate(d.getDate() + plusDays);
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    return `${d.getFullYear()}-${mm}-${dd}`;
  };

  const serving = (series: CalendarEvent | null) =>
    invokeMock.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === 'get_event_by_id' ? series : {}),
    );
  const written = () => invokeMock.mock.calls.find((call) => call[0] === 'update_event')?.[1].event;

  beforeEach(() => {
    invokeMock.mockReset();
    serving(master);
  });

  it('moves the start, the rule, the end date and the exceptions by the same number of days', async () => {
    // Dragged from a later Monday: the series must not start there.
    const occ = occurrenceOf(master, '2026-07-06T07:00:00.000Z');
    expect(await moveEventToDay(occ, localKey(occ.start, 1), 'series')).toBe(true);
    const row = written();
    expect(row.id).toBe('e1');
    expect(localKey(row.start)).toBe(localKey(master.start, 1));
    expect(row.recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=TU;UNTIL=20260901T235959Z');
    expect(row.recurrence.exceptions.map((e: string) => localKey(e))).toEqual([
      localKey('2026-06-29T07:00:00.000Z', 1),
    ]);
  });

  it('a new time of day keeps the days and moves the exceptions to that time', async () => {
    const occ = occurrenceOf(master, '2026-07-06T07:00:00.000Z');
    expect(await moveEventToSlot(occ, localKey(occ.start), 10 * 60 + 30, 'series')).toBe(true);
    const row = written();
    // Same weekday; the UTC end bound moves by as much as the start did.
    const moved = Date.parse(row.start) - Date.parse(master.start);
    const until = new Date(Date.parse('2026-08-31T23:59:59.000Z') + moved)
      .toISOString()
      .replace(/[-:]/g, '')
      .replace(/\.\d{3}/, '');
    expect(row.recurrence.rrule).toBe(`FREQ=WEEKLY;BYDAY=MO;UNTIL=${until}`);
    expect(localKey(row.start)).toBe(localKey(master.start));
    expect([new Date(row.start).getHours(), new Date(row.start).getMinutes()]).toEqual([10, 30]);
    const exception = new Date(row.recurrence.exceptions[0]);
    expect([exception.getHours(), exception.getMinutes()]).toEqual([10, 30]);
    expect(new Date(row.end).getTime() - new Date(row.start).getTime()).toBe(30 * 60_000);
  });

  it('reads the rule on the series clock, where a new time can fall on another day', async () => {
    // 23:00 on a Monday in Kiritimati (UTC+14). Two hours later it is Tuesday
    // there, while the device (UTC on CI, Berlin here) still shows the same day.
    const zoned = {
      ...master,
      start: '2026-06-15T09:00:00.000Z',
      end: '2026-06-15T09:30:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=MO',
        exceptions: ['2026-06-22T09:00:00.000Z'],
        tzid: 'Pacific/Kiritimati',
      },
    } as unknown as CalendarEvent;
    serving(zoned);
    const dropped = new Date('2026-06-15T11:00:00.000Z');
    const minute = dropped.getHours() * 60 + dropped.getMinutes();
    expect(await moveEventToSlot(zoned, localKey(dropped.toISOString()), minute, 'series')).toBe(true);
    const row = written();
    expect(row.start).toBe('2026-06-15T11:00:00.000Z');
    expect(row.recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=TU');
    expect(row.recurrence.exceptions).toEqual(['2026-06-22T11:00:00.000Z']);
    expect(row.recurrence.tzid).toBe('Pacific/Kiritimati');
  });

  it('places an hour-grid drop by the dragged row on the series clock', async () => {
    // A New York series written in winter; the March occurrence is in summer
    // time. Placing the drop on the January date with the device's clock put
    // the series an hour off.
    const zoned = {
      ...master,
      start: '2026-01-05T14:00:00.000Z',
      end: '2026-01-05T14:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO', exceptions: [], tzid: 'America/New_York' },
    } as unknown as CalendarEvent;
    serving(zoned);
    const occ = occurrenceOf(zoned, '2026-03-16T13:00:00.000Z');
    const dropped = new Date('2026-03-17T13:00:00.000Z'); // 09:00 in New York
    const minute = dropped.getHours() * 60 + dropped.getMinutes();
    expect(await moveEventToSlot(occ, localKey(dropped.toISOString()), minute, 'series')).toBe(true);
    const row = written();
    expect(row.start).toBe('2026-01-06T14:00:00.000Z');
    expect(row.recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=TU');
  });

  it('keeps the series time of day on a day-only drag across a clock change', async () => {
    // Fridays 09:00 in New York from a winter-time Friday; New York changes its
    // clocks between that Friday and the Monday the series moves to.
    const zoned = {
      ...master,
      start: '2026-03-06T14:00:00.000Z',
      end: '2026-03-06T14:30:00.000Z',
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=FR', exceptions: [], tzid: 'America/New_York' },
    } as unknown as CalendarEvent;
    serving(zoned);
    const occ = occurrenceOf(zoned, '2026-03-13T13:00:00.000Z');
    expect(await moveEventToDay(occ, localKey(occ.start, 3), 'series')).toBe(true);
    const row = written();
    // Monday 09:00 in New York, now in summer time.
    expect(row.start).toBe('2026-03-09T13:00:00.000Z');
    expect(row.recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=MO');
  });

  it('moves a UTC end bound with a new time of day', async () => {
    const bounded = {
      ...master,
      recurrence: { rrule: 'FREQ=WEEKLY;BYDAY=MO;UNTIL=20260831T070000Z', exceptions: [] },
    } as unknown as CalendarEvent;
    serving(bounded);
    const occ = occurrenceOf(bounded, '2026-07-06T07:00:00.000Z');
    const dropped = new Date('2026-07-06T08:00:00.000Z');
    const minute = dropped.getHours() * 60 + dropped.getMinutes();
    expect(await moveEventToSlot(occ, localKey(occ.start), minute, 'series')).toBe(true);
    // The last Monday stays in: the bound moved by the same hour.
    expect(written().recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=MO;UNTIL=20260831T080000Z');
  });

  it('moves a UTC end bound on the series clock across a clock change', async () => {
    // Mondays 09:00 in Berlin, bounded just before October 26 (winter time),
    // moved a week earlier: the bound keeps its Berlin reading a week earlier,
    // now in summer time, so the series ends on October 12.
    const berlin = {
      ...master,
      start: '2026-09-07T07:00:00.000Z',
      end: '2026-09-07T07:30:00.000Z',
      recurrence: {
        rrule: 'FREQ=WEEKLY;BYDAY=MO;UNTIL=20261026T075959Z',
        exceptions: [],
        tzid: 'Europe/Berlin',
      },
    } as unknown as CalendarEvent;
    serving(berlin);
    expect(await moveEventToDay(berlin, localKey(berlin.start, -7), 'series')).toBe(true);
    expect(written().recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=MO;UNTIL=20261019T065959Z');
  });

  it('refuses a rule that cannot move by whole days and writes nothing', async () => {
    const secondMonday = {
      ...master,
      recurrence: { rrule: 'FREQ=MONTHLY;BYDAY=2MO', exceptions: [] },
    } as unknown as CalendarEvent;
    serving(secondMonday);
    const occ = occurrenceOf(secondMonday, '2026-07-13T07:00:00.000Z');
    const move = moveEventToDay(occ, localKey(occ.start, 1), 'series');
    await expect(move).rejects.toBeInstanceOf(SeriesShiftRefusedError);
    await expect(move).rejects.toMatchObject({ reason: 'ordinal_weekday' });
    expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(false);
  });

  it('moves a dragged master row without loading it again', async () => {
    expect(await moveEventToDay(master, localKey(master.start, 2), 'series')).toBe(true);
    expect(invokeMock.mock.calls.some((call) => call[0] === 'get_event_by_id')).toBe(false);
    expect(written().recurrence.rrule).toBe('FREQ=WEEKLY;BYDAY=WE;UNTIL=20260902T235959Z');
  });

  it('writes nothing when the series cannot be loaded', async () => {
    serving(null);
    const occ = occurrenceOf(master, '2026-07-06T07:00:00.000Z');
    await expect(moveEventToDay(occ, localKey(occ.start, 1), 'series')).rejects.toThrow(/could not be loaded/);
    expect(invokeMock.mock.calls.some((call) => call[0] === 'update_event')).toBe(false);
  });
});

describe('moveEventToSlot (dropping an event in the hour grid)', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({});
  });

  const localKey = (iso: string, plusDays = 0) => {
    const d = new Date(iso);
    d.setDate(d.getDate() + plusDays);
    const mm = String(d.getMonth() + 1).padStart(2, '0');
    const dd = String(d.getDate()).padStart(2, '0');
    return `${d.getFullYear()}-${mm}-${dd}`;
  };

  const eventAt = (start: string, end: string, allDay = false) =>
    ({
      id: 'e1',
      calendar_id: 'c1',
      title: 'Standup',
      start,
      end,
      all_day: allDay,
      recurrence: null,
    }) as unknown as CalendarEvent;

  const written = () => invokeMock.mock.calls[0][1].event;

  it('carries the DURATION rather than recomputing it', async () => {
    // A drop names a START. An event that grew or shrank because it was
    // dragged would be a bug wearing a feature's clothes.
    const ev = eventAt('2026-06-15T09:00:00.000Z', '2026-06-15T10:30:00.000Z');
    expect(await moveEventToSlot(ev, localKey(ev.start), 14 * 60)).toBe(true);
    const row = written();
    expect(new Date(row.end).getTime() - new Date(row.start).getTime()).toBe(
      90 * 60_000,
    );
    expect(new Date(row.start).getHours()).toBe(14);
    expect(new Date(row.start).getMinutes()).toBe(0);
  });

  it('does nothing when neither the day nor the time changed', async () => {
    // The "dragged three pixels" misfire. A no-op write is not free: it bumps
    // updated_at at the provider and can lose a concurrent edit.
    const ev = eventAt('2026-06-15T09:00:00.000Z', '2026-06-15T10:00:00.000Z');
    const start = new Date(ev.start);
    const same = start.getHours() * 60 + start.getMinutes();
    expect(await moveEventToSlot(ev, localKey(ev.start), same)).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('moves the day and the time together', async () => {
    const ev = eventAt('2026-06-15T09:00:00.000Z', '2026-06-15T10:00:00.000Z');
    const target = localKey(ev.start, 3);
    expect(await moveEventToSlot(ev, target, 7 * 60 + 30)).toBe(true);
    const row = written();
    expect(localKey(row.start)).toBe(target);
    expect(new Date(row.start).getHours()).toBe(7);
    expect(new Date(row.start).getMinutes()).toBe(30);
  });

  it('ignores the minute for an all-day event', async () => {
    // It has no time to place, and turning it into a timed event is a much
    // bigger decision than a drag can carry.
    const ev = eventAt('2026-06-15T00:00:00.000Z', '2026-06-16T00:00:00.000Z', true);
    const target = localKey(ev.start, 3);
    expect(await moveEventToSlot(ev, target, 15 * 60)).toBe(true);
    const row = written();
    expect(new Date(row.start).getHours()).toBe(new Date(ev.start).getHours());
    expect(localKey(row.start)).toBe(target);
  });

  it('a same-day drop with no minute is still a no-op', async () => {
    // `moveEventToDay` is this function with `null`, so its old contract has
    // to survive the generalisation.
    const ev = eventAt('2026-06-15T09:00:00.000Z', '2026-06-15T10:00:00.000Z');
    expect(await moveEventToSlot(ev, localKey(ev.start), null)).toBe(false);
  });
});
