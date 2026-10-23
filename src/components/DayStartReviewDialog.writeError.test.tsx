import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { Task } from '../api/types';

/**
 * The day-start review's buttons say a write that failed (decision 273): it
 * used to go to the console alone, and the row simply stayed. A batch where
 * part landed says so, never "nothing was changed" (277).
 */

const { invokeMock, announce, invalidateData } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  announce: vi.fn(),
  invalidateData: vi.fn(),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));

/** Yesterday on the device's calendar, as the review counts days. */
function yesterday(): string {
  const day = new Date();
  day.setDate(day.getDate() - 1);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${day.getFullYear()}-${pad(day.getMonth() + 1)}-${pad(day.getDate())}`;
}

const slipped = (id: string, title: string) =>
  ({
    id,
    list_id: 'L1',
    title,
    description: null,
    status: 'open',
    priority: 'medium',
    effort: 'medium',
    scheduled_date: yesterday(),
    scheduled_time: null,
    scheduled_end_time: null,
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
    sound: null,
    assignees: [],
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    completed_at: null,
    etag: null,
  }) as unknown as Task;

const TASKS = { rows: [] as Task[] };
const LISTS = new Map([['L1', { id: 'L1', name: 'Haushalt', task_capabilities: {} }]]);
vi.mock('../state/useTasks', () => ({
  useTasks: () => ({ tasks: TASKS.rows, taskListById: LISTS, loading: false }),
}));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ invalidateData, openTaskDialog: () => {} }),
}));
const CASCADE = {
  effectiveForList: () => ({ cascade: false, autoDate: false }),
  priorityScale: 'three',
  remindUntimedToday: false,
  remindDeadlineArrived: false,
  remindDeadlineCountdown: false,
  deadlineCountdownDays: 3,
};
vi.mock('../state/taskCascadeContext', () => ({ useTaskCascadeEnabled: () => CASCADE }));
const STATUS = { set: async () => {} };
vi.mock('../state/useTaskStatusToggle', () => ({ useTaskStatusActions: () => STATUS }));
const NOBODY = {};
vi.mock('../state/currentUser', () => ({ useCurrentUserByList: () => NOBODY }));

const BUSY = { code: 'forbidden', message: 'provider-busy: http-429' };

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockReset();
  announce.mockClear();
  invalidateData.mockClear();
});

async function open(rows: Task[]) {
  TASKS.rows = rows;
  const { DayStartReviewDialog } = await import('./DayStartReviewDialog');
  render(<DayStartReviewDialog isOpen onClose={() => {}} />);
}

describe('DayStartReviewDialog → a write that failed', () => {
  it('says a busy "carry to today" in words', async () => {
    invokeMock.mockRejectedValue(BUSY);
    await open([slipped('t1', 'Einkaufen')]);
    fireEvent.click(await screen.findByRole('button', { name: 'Heute' }));
    await waitFor(() =>
      expect(announce).toHaveBeenCalledWith(
        'Der Anbieter ist gerade ausgelastet. Es wurde nichts geändert. Versuche es später noch einmal.',
      ),
    );
    expect(invalidateData).toHaveBeenCalled();
    // The row stays: nothing of it moved.
    expect(screen.getByRole('button', { name: 'Heute' })).toBeTruthy();
  });

  it('says a batch that landed in part as partly saved', async () => {
    invokeMock.mockImplementation((_cmd: string, args: { task: Task }) =>
      args.task.id === 't1' ? Promise.resolve(args.task) : Promise.reject(BUSY),
    );
    await open([slipped('t1', 'Einkaufen'), slipped('t2', 'Putzen')]);
    fireEvent.click(await screen.findByRole('button', { name: 'Alle auf heute' }));
    await waitFor(() =>
      expect(announce).toHaveBeenCalledWith(
        'Die Änderung ist nur zum Teil gespeichert, weil der Anbieter gerade ausgelastet ist. ' +
          'Prüfe sie und versuche den Rest später noch einmal.',
      ),
    );
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});
