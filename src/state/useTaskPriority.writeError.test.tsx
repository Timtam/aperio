import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { writeErrorMessage } from '@aperio/shared';

import type { Task } from '../api/types';
import i18n from '../i18n';

/**
 * A priority change that failed reads the view again (decision 277), as a
 * failed status change from the same menu does: a save whose answer was lost,
 * or one that stands in part (Vikunja's assignees after the fields), may have
 * landed, and the row and the menu's check mark would keep the old priority.
 */

const { invokeMock, invalidateData, announced } = vi.hoisted(() => ({
  invokeMock: vi.fn((_command: string) => Promise.reject({ code: 'network', message: 'reset' })),
  invalidateData: vi.fn(),
  announced: [] as string[],
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));
vi.mock('./dialogStateContext', () => ({ useDialogState: () => ({ invalidateData }) }));
vi.mock('./taskCascadeContext', () => ({
  useTaskCascadeEnabled: () => ({ priorityScale: 'three' }),
}));

import { useTaskPriorityAction } from './useTaskPriority';

const TASK = { id: 't-1', list_id: 'L1', title: 'Bericht', priority: 'medium' } as unknown as Task;
const t = (key: string, values?: Record<string, unknown>): string =>
  i18n.t(key, values as never) as unknown as string;

afterEach(() => {
  invokeMock.mockClear();
  invalidateData.mockClear();
  announced.length = 0;
});

describe('a priority change that failed (277)', () => {
  it('says the failure and reads the view again', async () => {
    const { result } = renderHook(() => useTaskPriorityAction());
    await act(async () => {
      await result.current(TASK, 'high');
    });
    expect(announced).toEqual([writeErrorMessage({ code: 'network', message: 'reset' }, t)]);
    expect(invalidateData).toHaveBeenCalledTimes(1);
  });
});
