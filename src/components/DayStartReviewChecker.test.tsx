import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';

/**
 * The review a silent carry-over batch opens once it is done: not on top of a
 * dialog that opened while the batch ran — a write's wait, a sync prompt —,
 * but once that dialog is closed.
 */

const { dialog, gate, planned } = vi.hoisted(() => ({
  dialog: {
    mode: { kind: 'none' } as { kind: string },
    openDayStartReview: vi.fn(),
    invalidateData: vi.fn(),
  },
  gate: { release: () => {} },
  planned: { surfaced: 1 },
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(
    () =>
      new Promise((resolve) => {
        gate.release = () => resolve({});
      }),
  ),
}));
vi.mock('@aperio/shared', () => ({
  planDayStart: () => ({
    todayRows: [{ id: 't1' }],
    backlogRows: [],
    todayTargets: [{ id: 't1', list_id: 'l1', scheduled_date: '2026-10-17' }],
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
vi.mock('../state/toastContext', () => ({ useToast: () => ({ showToast: () => '' }) }));
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
