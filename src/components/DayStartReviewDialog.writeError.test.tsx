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
const STATUS = {
  set: async (_task: Task, _status: string, _say?: (sentence: string) => void) => true,
};
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

async function open(rows: Task[], onClose: () => void = () => {}) {
  TASKS.rows = rows;
  const { DayStartReviewDialog } = await import('./DayStartReviewDialog');
  render(<DayStartReviewDialog isOpen onClose={onClose} />);
}

const PARTLY =
  'Die Änderung ist nur zum Teil gespeichert, weil der Anbieter gerade ausgelastet ist. ' +
  'Prüfe sie und versuche den Rest später noch einmal.';

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

  it('keeps a row whose completion was turned away', async () => {
    // The shared action said the failure; the row stays to be tried again,
    // and the review does not close as handled.
    STATUS.set = async () => false;
    await open([slipped('t1', 'Einkaufen')]);
    fireEvent.click(await screen.findByRole('button', { name: 'Erledigt' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Erledigt' })).toBeTruthy());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByRole('button', { name: 'Erledigt' })).toBeTruthy();
    expect(announce).not.toHaveBeenCalledWith('Alle Einträge bearbeitet.');
    STATUS.set = async () => true;
  });

  it("keeps focus in the dialog when a failed Done's re-read takes the row away", async () => {
    // The completion landed, a later step did not: set() says the failure
    // and reports no success, and the re-read no longer lists the task.
    STATUS.set = async () => {
      TASKS.rows = TASKS.rows.filter((row) => row.id !== 't1');
      return false;
    };
    await open([slipped('t1', 'Einkaufen'), slipped('t2', 'Putzen')]);
    const [first] = await screen.findAllByRole('button', { name: 'Erledigt' });
    first.focus();
    fireEvent.click(first);
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'Erledigt' })).toHaveLength(1));
    const dialog = screen.getByRole('dialog');
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    STATUS.set = async () => true;
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

  it('keeps a failure on screen when the action that cleared the last row stood in part', async () => {
    // Done on the last row: the status landed, a later step of the save did
    // not. "All handled" would speak over the sentence and close with it
    // unseen; the dialog shows both with Close only (Toni, 2026-10-24).
    STATUS.set = async (_task, _status, say) => {
      say?.(PARTLY);
      return true;
    };
    const onClose = vi.fn();
    await open([slipped('t1', 'Einkaufen')], onClose);
    fireEvent.click(await screen.findByRole('button', { name: 'Erledigt' }));
    expect(await screen.findByText(`${PARTLY} Alle Einträge bearbeitet.`)).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(announce).not.toHaveBeenCalledWith('Alle Einträge bearbeitet.');
    fireEvent.click(screen.getByRole('button', { name: 'Schließen' }));
    expect(onClose).toHaveBeenCalledTimes(1);
    STATUS.set = async () => true;
  });

  it('keeps a batch failure on screen when every row it showed landed', async () => {
    // "All to today" with cascade: the row moved, its hidden subtask was
    // turned away busy.
    CASCADE.effectiveForList = () => ({ cascade: true, autoDate: false });
    invokeMock.mockImplementation((_cmd: string, args: { task: Task }) =>
      args.task.id === 'p1' ? Promise.resolve(args.task) : Promise.reject(BUSY),
    );
    const parent = slipped('p1', 'Umzug');
    const child = { ...slipped('c1', 'Kartons'), parent_id: 'p1' } as Task;
    const onClose = vi.fn();
    await open([parent, child], onClose);
    fireEvent.click(await screen.findByRole('button', { name: 'Alle auf heute' }));
    expect(await screen.findByText(`${PARTLY} Alle Einträge bearbeitet.`)).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    CASCADE.effectiveForList = () => ({ cascade: false, autoDate: false });
  });

  it('closes as handled where the action that cleared the last row worked', async () => {
    const onClose = vi.fn();
    await open([slipped('t1', 'Einkaufen')], onClose);
    fireEvent.click(await screen.findByRole('button', { name: 'Erledigt' }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(announce).toHaveBeenCalledWith('Alle Einträge bearbeitet.');
  });
});
