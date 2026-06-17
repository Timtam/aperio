import { useEffect, useMemo, useState } from 'react';

import type { Task } from '@aperio/shared';

import { getTasks } from '../api/client';
import { useTaskStore } from './taskStoreContext';

/**
 * Pull tasks from every selected task list and return the aggregated list.
 *
 * Ported byte-for-byte from the desktop's `src/state/useTasks.ts` — same
 * module-level stale-while-revalidate cache, same fan-out + "partial result on
 * per-list failure" policy, same sort order, same cache key (the sorted
 * task-list ids joined by a space). The only difference: it reads the
 * selection + invalidation counter from the mobile `TaskStore` instead of the
 * desktop's `CalendarStore` + `DialogState`.
 *
 * Sorting key: tasks with a concrete scheduled date come first (ordered by
 * date), tasks with only a deadline next, undated tasks last.
 */

type CacheKey = string;

const tasksCache = new Map<CacheKey, Task[]>();
let cachedDataVersion = -1;

function ensureCacheVersion(version: number): void {
  if (version !== cachedDataVersion) {
    tasksCache.clear();
    cachedDataVersion = version;
  }
}

function cacheGet(key: CacheKey, version: number): Task[] | undefined {
  ensureCacheVersion(version);
  return tasksCache.get(key);
}

function cacheSet(key: CacheKey, version: number, tasks: Task[]): void {
  ensureCacheVersion(version);
  tasksCache.set(key, tasks);
}

/** Test-only escape hatch — wipes the cache so each test starts clean. */
export function __resetTasksCacheForTests(): void {
  tasksCache.clear();
  cachedDataVersion = -1;
}

export function useTasks() {
  const { selectedTaskListIds, taskLists, taskListsLoading, dataVersion } =
    useTaskStore();

  const idsKey = useMemo(
    () => [...selectedTaskListIds].sort().join(' '),
    [selectedTaskListIds],
  );

  // Lazy init: read cache before the first paint so a remount with a
  // previously seen list selection comes back with data already.
  const [tasks, setTasks] = useState<Task[]>(
    () => cacheGet(idsKey, dataVersion) ?? [],
  );
  const [loading, setLoading] = useState<boolean>(
    () => cacheGet(idsKey, dataVersion) === undefined,
  );

  useEffect(() => {
    let cancelled = false;

    const cached = cacheGet(idsKey, dataVersion);
    if (cached) {
      setTasks(cached);
      setLoading(false);
    } else {
      setLoading(true);
    }

    // Wait only for the TASK-LIST catalog before deciding anything else.
    if (taskListsLoading) return;

    const ids = [...selectedTaskListIds];
    if (ids.length === 0) {
      setTasks([]);
      setLoading(false);
      cacheSet(idsKey, dataVersion, []);
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
      cacheSet(idsKey, dataVersion, flat);
      setTasks(flat);
      setLoading(false);
    });

    return () => {
      cancelled = true;
    };
    // selectedTaskListIds intentionally omitted — `idsKey` is the
    // stable projection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskListsLoading, idsKey, dataVersion]);

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
