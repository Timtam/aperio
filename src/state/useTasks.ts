import { useEffect, useMemo, useState } from 'react';

import { getTasks } from '../api/client';
import type { Task } from '../api/types';
import { useCalendarStore } from './calendarStoreContext';
import { useDialogState } from './dialogStateContext';

/**
 * Pull tasks from every selected task list and return the aggregated list.
 *
 * Mirrors the shape of `useEvents` — same fan-out strategy, same
 * per-container retention policy. Sorting key: tasks with a concrete
 * scheduled date come first (ordered by date), tasks with only a
 * deadline next, undated tasks last.
 *
 * Stale-while-revalidate cache: see `useEvents` for the full rationale.
 * The aggregate entry is KEPT across `dataVersion` bumps and served
 * stale while the refetch runs (the contract useEvents documents — an
 * earlier version wiped the whole cache on every bump, which turned each
 * of the many app-start bumps into a cold refetch and let a shrunken
 * batch replace a fuller one). A per-list layer additionally retains
 * each list's last successful batch, so a transiently failing list keeps
 * its previous tasks on screen instead of shrinking the aggregate — and
 * with it the day-entry count screen readers announce.
 *
 * The version guard is still monotonic: a refetch that resolves late,
 * carrying a superseded `dataVersion`, is dropped so it can't overwrite
 * post-mutation data with a pre-mutation snapshot.
 */

type CacheKey = string;

const tasksCache = new Map<CacheKey, Task[]>();
/** Last successful batch per task-list id (see `perCalendarCache`). */
const perListCache = new Map<string, Task[]>();
/** Highest dataVersion any effect run has seen — stale-write fence. */
let latestVersion = -1;

function cacheGet(key: CacheKey): Task[] | undefined {
  return tasksCache.get(key);
}

function cacheSet(key: CacheKey, version: number, tasks: Task[]): void {
  // Drop a write from a superseded refetch: its batch predates a mutation that
  // already bumped the version, so it must not overwrite the current data.
  if (version < latestVersion) return;
  tasksCache.set(key, tasks);
}

/** Test-only escape hatch — wipes the caches so each test starts clean. */
export function __resetTasksCacheForTests(): void {
  tasksCache.clear();
  perListCache.clear();
  latestVersion = -1;
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
  const [tasks, setTasks] = useState<Task[]>(() => cacheGet(idsKey) ?? []);
  const [loading, setLoading] = useState<boolean>(
    () => cacheGet(idsKey) === undefined,
  );

  useEffect(() => {
    let cancelled = false;
    if (dataVersion > latestVersion) latestVersion = dataVersion;

    const cached = cacheGet(idsKey);
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

    let failures = 0;
    Promise.all(
      ids.map((id) =>
        getTasks(id).then(
          (batch) => {
            if (dataVersion >= latestVersion) perListCache.set(id, batch);
            return batch;
          },
          (err) => {
            // A transient per-list failure keeps the list's last known
            // batch instead of shrinking the aggregate (same policy as
            // useEvents' per-calendar retention).
            // eslint-disable-next-line no-console
            console.warn('get_tasks failed for list', id, err);
            failures += 1;
            return perListCache.get(id) ?? ([] as Task[]);
          },
        ),
      ),
    ).then((batches) => {
      // `cancelled` covers the effect re-running; the version check additionally
      // drops a fetch that a newer bump superseded mid-flight (the calendar
      // views churn dataVersion via background event refreshes, widening this
      // window) so a stale read can't replace the fresh task set.
      if (cancelled || dataVersion < latestVersion) return;
      const flat = batches.flat();
      flat.sort(taskOrder);
      // Only a failure-free run may become the authoritative cache entry
      // (a failure-patched aggregate would later be served as fresh).
      if (failures === 0) cacheSet(idsKey, dataVersion, flat);
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
