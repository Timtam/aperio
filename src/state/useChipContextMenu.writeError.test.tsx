import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { eventWriteErrorMessage, writeErrorMessage } from '@aperio/shared';

import type { Calendar, CalendarEvent, Task } from '../api/types';
import i18n from '../i18n';

/**
 * A write from a chip's or a row's context menu that failed reads the view
 * again (decision 277): a delete may have landed in part — a calendar walk
 * that lost an answer, a meeting cancelled before its DELETE failed — and the
 * view's own Delete key already reads again. Before, the menu only said the
 * failure, and the grid kept showing what may be gone.
 */

const { invokeMock, invalidateData, announced, menuPick, writeFails } = vi.hoisted(() => {
  const announced: string[] = [];
  /** The entry the native menu answers with. */
  const menuPick: { current: string | null } = { current: null };
  /** What every write fails with. */
  const writeFails: { current: unknown } = { current: null };
  const invokeMock = vi.fn((command: string) => {
    if (command === 'show_context_menu') return Promise.resolve(menuPick.current);
    if (command === 'event_groups_for_events') return Promise.resolve([]);
    if (['delete_event', 'set_event_color', 'update_event', 'delete_task'].includes(command)) {
      return Promise.reject(writeFails.current);
    }
    return Promise.resolve(null);
  });
  return { invokeMock, invalidateData: vi.fn(), announced, menuPick, writeFails };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));
vi.mock('./dialogStateContext', () => ({
  useDialogState: () => ({
    openEventDialog: () => {},
    openTaskDialog: () => {},
    openMoveCopy: () => {},
    openEventGroup: () => {},
    openPlanTask: () => {},
    invalidateData,
  }),
}));
const CALENDAR = {
  id: 'work',
  name: 'Arbeit',
  read_only: false,
  account_id: 'acc-google',
} as unknown as Calendar;
const STORE = { colorLabels: [{ id: 'blue', name: 'Blau', ad_hoc: false }], calendars: [CALENDAR] };
vi.mock('./calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('./useTaskStatusToggle', () => ({ useTaskStatusActions: () => ({ set: vi.fn() }) }));
vi.mock('./useTaskPriority', () => ({ useTaskPriorityAction: () => vi.fn() }));
vi.mock('./taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({ priorityScale: 'two-level' }),
}));

import { useChipContextMenu } from './useChipContextMenu';

const EVENT = {
  id: 'ev-1',
  calendar_id: 'work',
  title: 'Planung',
  description: null,
  location: null,
  start: '2026-06-15T08:00:00.000Z',
  end: '2026-06-15T09:00:00.000Z',
  all_day: false,
  recurrence: null,
  color_label: null,
  reminders: [],
  attendees: [],
} as unknown as CalendarEvent;

const TASK = {
  id: 't-1',
  list_id: 'L1',
  title: 'Bericht',
  status: 'open',
  priority: 'medium',
  parent_id: null,
  assignees: [],
} as unknown as Task;

afterEach(() => {
  invokeMock.mockClear();
  invalidateData.mockClear();
  announced.length = 0;
  menuPick.current = null;
  writeFails.current = null;
});

const LOST = { code: 'network', message: 'connection reset' };
const t = (key: string, values?: Record<string, unknown>): string =>
  i18n.t(key, values as never) as unknown as string;

describe('the context menu after a write that failed (277)', () => {
  it.each(['delete', 'color:blue'])('reads the view again after an event %s failed', async (pick) => {
    menuPick.current = pick;
    writeFails.current = LOST;
    const { result } = renderHook(() => useChipContextMenu());
    await act(async () => {
      await result.current.openForEvent(EVENT);
    });
    expect(announced).toEqual([eventWriteErrorMessage(LOST, t)]);
    expect(invalidateData).toHaveBeenCalledTimes(1);
  });

  it('reads the view again after a task delete failed', async () => {
    menuPick.current = 'delete';
    writeFails.current = LOST;
    const { result } = renderHook(() => useChipContextMenu());
    await act(async () => {
      await result.current.openForTask(TASK);
    });
    expect(announced).toEqual([writeErrorMessage(LOST, t)]);
    expect(invalidateData).toHaveBeenCalledTimes(1);
  });
});
