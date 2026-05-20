import { useEffect, useMemo, useState } from 'react';

import { getTasks } from '../api/client';
import type { Task } from '../api/types';
import { useCalendarStore } from './CalendarStore';
import { useDialogState } from './DialogState';

/**
 * Pull tasks from every selected task list and return the aggregated list.
 *
 * Mirrors the shape of `useEvents` — same fan-out strategy, same
 * "partial result on per-list failure" policy. Sorting key: tasks with a
 * concrete scheduled date come first (ordered by date), tasks with only
 * a deadline next, undated tasks last.
 */
export function useTasks() {
  const { selectedTaskListIds, taskLists, loading: storeLoading } =
    useCalendarStore();
  const { dataVersion } = useDialogState();
  const [tasks, setTasks] = useState<Task[]>([]);
  // True until the first fetch settles, then stays false. Subsequent
  // refetches keep the previously loaded tasks on screen — see
  // useEvents for the full reasoning.
  const [loading, setLoading] = useState(true);

  // Re-fetch when any mutation hint fires — see useEvents for the rationale.

  const idsKey = useMemo(
    () => [...selectedTaskListIds].sort().join(' '),
    [selectedTaskListIds],
  );

  useEffect(() => {
    let cancelled = false;

    // Mirror useEvents: hold off until the task-list catalog has loaded.
    if (storeLoading) return;

    const ids = [...selectedTaskListIds];
    if (ids.length === 0) {
      setTasks([]);
      setLoading(false);
      return;
    }

    Promise.all(
      ids.map((id) =>
        getTasks(id).catch((err) => {
          // eslint-disable-next-line no-console
          console.warn('get_tasks failed for list', id, err);
          return [] as Task[];
        }),
      ),
    ).then((batches) => {
      if (cancelled) return;
      const flat = batches.flat();
      flat.sort(taskOrder);
      setTasks(flat);
      setLoading(false);
    });

    return () => {
      cancelled = true;
    };
  }, [storeLoading, idsKey, selectedTaskListIds, dataVersion]);

  const taskListById = useMemo(() => {
    const map = new Map<string, (typeof taskLists)[number]>();
    taskLists.forEach((l) => map.set(l.id, l));
    return map;
  }, [taskLists]);

  return { tasks, loading, taskListById };
}

function taskOrder(a: Task, b: Task): number {
  // Bucket: 0 = has scheduled date, 1 = has deadline only, 2 = neither.
  const bucketA = a.scheduled_date ? 0 : a.deadline_date ? 1 : 2;
  const bucketB = b.scheduled_date ? 0 : b.deadline_date ? 1 : 2;
  if (bucketA !== bucketB) return bucketA - bucketB;

  const dateA = a.scheduled_date ?? a.deadline_date ?? '';
  const dateB = b.scheduled_date ?? b.deadline_date ?? '';
  if (dateA !== dateB) return dateA.localeCompare(dateB);

  return a.created_at.localeCompare(b.created_at);
}
