import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

/**
 * The task offers run two search passes and join them. Two task servers
 * count from the same 1, so the join has to tell rows apart by list AND id:
 * by id alone, a task from the second server vanished behind the first.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((_command: string, args?: { filters?: { task_statuses?: string[] } | null }) => {
    const row = (list_id: string) => ({ id: '1', list_id, title: 'Einkaufen' });
    const live = args?.filters?.task_statuses != null;
    return Promise.resolve({
      events: [],
      tasks: live ? [row('vikunja-a')] : [row('vikunja-a'), row('vikunja-b')],
    });
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

describe('useTitleSuggestions for tasks', () => {
  it('keeps two tasks with one id from two lists', async () => {
    const { useTitleSuggestions } = await import('./useTitleSuggestions');
    const { result } = renderHook(() => useTitleSuggestions('einkaufen', 'tasks', true));
    await waitFor(() => expect(result.current).toHaveLength(2));
    expect(result.current.map((t) => (t as { list_id: string }).list_id)).toEqual([
      'vikunja-a',
      'vikunja-b',
    ]);
  });
});
