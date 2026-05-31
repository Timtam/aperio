import { createContext, useContext } from 'react';

import type { TaskCascadeContextValue } from './TaskCascadeProvider';

/**
 * Task-behaviour preferences context + consumer hook. Split out of
 * `TaskCascadeProvider` so that component file exports only its
 * component (Fast Refresh). The value type + the provider
 * implementation live alongside the component.
 */
export const TaskCascadeContext =
  createContext<TaskCascadeContextValue | null>(null);

/**
 * Read every task preference plus the hydration flag. Existing
 * consumers that destructure `{ enabled }` continue to work; new
 * consumers can pull `autoDate` and `carryOverDefault` from the same
 * call.
 */
export function useTaskCascadeEnabled(): TaskCascadeContextValue {
  const ctx = useContext(TaskCascadeContext);
  if (!ctx) {
    throw new Error(
      'useTaskCascadeEnabled must be used inside <TaskCascadeProvider>',
    );
  }
  return ctx;
}
