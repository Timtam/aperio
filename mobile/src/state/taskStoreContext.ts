import { createContext, useContext } from 'react';

import type { TaskStoreState } from './taskStore';

/**
 * Task store context + consumer hook. Split out of `TaskStoreProvider` so the
 * component file exports only its component (Fast Refresh) — mirrors the
 * desktop's `calendarStoreContext.ts` / `dialogStateContext.ts` split.
 */
export const TaskStoreContext = createContext<TaskStoreState | null>(null);

export function useTaskStore(): TaskStoreState {
  const ctx = useContext(TaskStoreContext);
  if (!ctx) {
    throw new Error('useTaskStore must be used inside <TaskStoreProvider>');
  }
  return ctx;
}
