import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { CalendarEvent, Task } from '../api/types';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import {
  EVENT_DND_TYPE,
  moveTaskToBacklog,
  readEventDrag,
  readTaskDrag,
  scheduleTaskOnDay,
  setEventDrag,
  setTaskDrag,
  TASK_DND_TYPE,
} from './moveActions';

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

  it('moveTaskToBacklog clears all dates and reopens a completed task', async () => {
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
    expect(task.deadline_date).toBeNull();
    expect(task.deadline_time).toBeNull();
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
