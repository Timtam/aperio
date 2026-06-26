import { useEffect, useMemo, useState } from 'react';

import { getTasks } from '../api/client';
import type { Task } from '../api/types';
import { useCalendarStore } from './calendarStoreContext';
import { useDialogState } from './dialogStateContext';

/**
 * Pull tasks from every selected task list and return the aggregated list.
 *
 * Mirrors the shape of `useEvents` — same fan-out strategy, same
 * "partial result on per-list failure" policy. Sorting key: tasks with a
 * concrete scheduled date come first (ordered by date), tasks with only
 * a deadline next, undated tasks last.
 *
 * Stale-while-revalidate cache: see `useEvents` for the full rationale.
 * The model is identical — the only difference is that tasks aren't
 * range-scoped, so the cache key is just the sorted task-list ids.
 */

type CacheKey = string;

const tasksCache = new Map<CacheKey, Task[]>();
let cachedDataVersion = -1;

function ensureCacheVersion(version: number): void {
  // Monotonic: only ever ADVANCE. dataVersion increments on every mutation, so
  // a refetch that resolves late with a stale closure version must never rewind
  // the cache — rewinding would clear the fresh batch and re-admit the stale
  // one. (`!==` allowed exactly that backward step.)
  if (version > cachedDataVersion) {
    tasksCache.clear();
    cachedDataVersion = version;
  }
}

function cacheGet(key: CacheKey, version: number): Task[] | undefined {
  ensureCacheVersion(version);
  return tasksCache.get(key);
}

function cacheSet(key: CacheKey, version: number, tasks: Task[]): void {
  // Drop a write from a superseded refetch: its batch predates a mutation that
  // already bumped the version, so it must not overwrite the current data.
  if (version < cachedDataVersion) return;
  ensureCacheVersion(version);
  tasksCache.set(key, tasks);
}

/** Test-only escape hatch — wipes the cache so each test starts clean. */
export function __resetTasksCacheForTests(): void {
  tasksCache.clear();
  cachedDataVersion = -1;
}

export function useTasks() {
  const { selectedTaskListIds, taskLists, taskListsLoading } =
    useCalendarStore();
  const { dataVersion } = useDialogState();

  const idsKey = useMemo(
    () => [...selectedTaskListIds].sort().join(' '),
    [selectedTaskListIds],
  );

  // Lazy init: read cache before the first paint so a remount with
  // a previously seen list selection comes back with data already.
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

    // Wait only for the TASK-LIST catalog before deciding anything else —
    // not the whole store. Tasks don't need calendars or contacts, so a
    // slow calendar enumeration must not delay the task view's first paint.
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
      // `cancelled` covers the effect re-running; the version check additionally
      // drops a fetch that a newer bump superseded mid-flight (the calendar
      // views churn dataVersion via background event refreshes, widening this
      // window) so a stale read can't replace the fresh task set.
      if (cancelled || dataVersion < cachedDataVersion) return;
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
