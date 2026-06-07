import { describe, expect, it } from 'vitest';

import type { CalendarEvent, Task } from '../api/types';
import {
  EVENT_DND_TYPE,
  readEventDrag,
  readTaskDrag,
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
