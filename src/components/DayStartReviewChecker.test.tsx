import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';

/**
 * The review a silent carry-over batch opens once it is done: not on top of a
 * dialog that opened while the batch ran — a write's wait, a sync prompt —,
 * but once that dialog is closed.
 */

const { dialog, gate, planned, gated, invokeMock, toast } = vi.hoisted(() => {
  const gate = { release: () => {} };
  const gated = () =>
    new Promise((resolve) => {
      gate.release = () => resolve({});
    });
  return {
    dialog: {
      mode: { kind: 'none' } as { kind: string },
      openDayStartReview: vi.fn(),
      invalidateData: vi.fn(),
    },
    gate,
    gated,
    invokeMock: { fn: gated as (cmd: string, args: unknown) => Promise<unknown> },
    planned: {
      surfaced: 1,
      todayTargets: [{ id: 't1', list_id: 'l1', scheduled_date: '2026-10-17' }],
    },
    toast: { last: null as null | { undo?: { action: () => Promise<void> | void } } },
  };
});

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args: unknown) => invokeMock.fn(cmd, args),
}));
vi.mock('@aperio/shared', async () => ({
  ...(await vi.importActual<typeof import('@aperio/shared')>('@aperio/shared')),
  planDayStart: () => ({
    todayRows: [{ id: 't1' }],
    backlogRows: [],
    todayTargets: planned.todayTargets,
    backlogTargets: [],
    reminders: { untimed: [], dueToday: [], countdown: [] },
    surfaced: planned.surfaced,
  }),
}));
vi.mock('../intl/taskDay', () => ({ todayIsoKey: () => '2026-10-18' }));
vi.mock('../state/useTasks', () => ({ useTasks: () => ({ tasks: [], loading: false }) }));
vi.mock('../state/currentUser', () => ({ useCurrentUserByList: () => ({}) }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => dialog }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/toastContext', () => ({
  useToast: () => ({
    showToast: (input: (typeof toast)['last']) => {
      toast.last = input;
      return '';
    },
  }),
}));
vi.mock('../state/notify', () => ({ notify: () => Promise.resolve() }));
vi.mock('./dayStartReview', () => ({ isDayStartReviewSnoozed: () => false }));
vi.mock('../hooks/useCurrentDayKey', () => ({
  readFiredDayKey: () => null,
  shouldFireToday: (_trigger: unknown, fired: string | null, today: string) => fired !== today,
  useCurrentDayKey: () => '2026-10-18',
  writeFiredDayKey: () => {},
}));
vi.mock('../state/taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({
    effectiveForList: () => ({}),
    dayStartTrigger: 'immediate',
    hydrating: false,
    remindUntimedToday: false,
    remindDeadlineArrived: false,
    remindDeadlineCountdown: false,
    deadlineCountdownDays: 3,
  }),
}));

import { DayStartReviewChecker } from './DayStartReviewChecker';

afterEach(() => {
  dialog.mode = { kind: 'none' };
  dialog.openDayStartReview.mockClear();
  dialog.invalidateData.mockClear();
  invokeMock.fn = gated;
  planned.todayTargets = [{ id: 't1', list_id: 'l1', scheduled_date: '2026-10-17' }];
  toast.last = null;
});

describe('the review after a silent carry-over', () => {
  it('opens once the batch is done when no dialog is open', async () => {
    render(<DayStartReviewChecker />);
    await act(async () => {
      gate.release();
    });
    expect(dialog.openDayStartReview).toHaveBeenCalledTimes(1);
  });

  it('waits for a dialog that opened while the batch ran', async () => {
    const view = render(<DayStartReviewChecker />);
    // A write's wait opens while the batch runs.
    dialog.mode = { kind: 'stepWrite' };
    view.rerender(<DayStartReviewChecker />);
    await act(async () => {
      gate.release();
    });
    expect(dialog.openDayStartReview).not.toHaveBeenCalled();
    // It closes: now the review opens, once.
    dialog.mode = { kind: 'none' };
    view.rerender(<DayStartReviewChecker />);
    expect(dialog.openDayStartReview).toHaveBeenCalledTimes(1);
    view.rerender(<DayStartReviewChecker />);
    expect(dialog.openDayStartReview).toHaveBeenCalledTimes(1);
  });
});

/**
 * The carry-over's Undo, which writes every task back (decisions 273, 277):
 * one turned away after others landed leaves the undo partly saved, never
 * "nothing was changed", and the rows read again.
 */
describe('the undo of a silent carry-over', () => {
  it('lands in part when one write is turned away', async () => {
    planned.todayTargets = [
      { id: 't1', list_id: 'l1', scheduled_date: '2026-10-17' },
      { id: 't2', list_id: 'l1', scheduled_date: '2026-10-17' },
    ];
    // The carry lands; the undo of t2 meets a busy provider.
    invokeMock.fn = (_cmd, args) => {
      const task = (args as { task: { id: string; scheduled_date: string } }).task;
      return task.id === 't2' && task.scheduled_date === '2026-10-17'
        ? Promise.reject({ code: 'forbidden', message: 'provider-busy: http-429' })
        : Promise.resolve({});
    };
    render(<DayStartReviewChecker />);
    await act(async () => {
      await Promise.resolve();
    });
    const undo = toast.last?.undo;
    expect(undo).toBeTruthy();
    dialog.invalidateData.mockClear();
    await expect(Promise.resolve(undo!.action())).rejects.toEqual({
      code: 'network',
      message: 'partly-saved: undo',
    });
    expect(dialog.invalidateData).toHaveBeenCalled();
  });
});
