import type { TaskUser } from '@aperio/shared';
import { useEffect, useMemo, useState } from 'react';

import { taskCurrentUser } from '../api/client';

// The connected user ("me") for a task list's account, used to self-assign on
// status change and to split / filter tasks by ownership. `current_user` is a
// per-account identity (Vikunja `GET /user`); we memoize it per list for the
// session — local / non-assignable backends resolve to `null` and the dependent
// features stay inert there. The mobile twin of the desktop src/state/currentUser.

const cache = new Map<string, Promise<TaskUser | null>>();

/** The connected user for `listId`'s account, memoized for the session. A list
 *  whose backend has no identity resolves to `null`. Best-effort: a failed
 *  lookup is cached as `null` rather than retried on every call. */
export function currentUserForList(listId: string): Promise<TaskUser | null> {
  let pending = cache.get(listId);
  if (!pending) {
    pending = taskCurrentUser(listId).catch(() => null);
    cache.set(listId, pending);
  }
  return pending;
}

/** Map `list_id → connected user` for the lists the given tasks belong to,
 *  populated lazily. Re-fetches only when the SET of lists changes, not on every
 *  render. Absent / null entries mean "no identity" (local lists). */
export function useCurrentUserByList(
  tasks: readonly { list_id: string }[],
): Record<string, TaskUser | null> {
  const [map, setMap] = useState<Record<string, TaskUser | null>>({});
  // A stable string key of the distinct list ids so the effect only re-runs
  // when the lists actually change (list ids never contain a newline).
  const signature = useMemo(
    () =>
      Array.from(new Set(tasks.map((task) => task.list_id)))
        .sort()
        .join('\n'),
    [tasks],
  );
  useEffect(() => {
    let cancelled = false;
    const ids = signature ? signature.split('\n') : [];
    void Promise.all(
      ids.map(async (id) => [id, await currentUserForList(id)] as const),
    ).then((entries) => {
      if (!cancelled) setMap(Object.fromEntries(entries));
    });
    return () => {
      cancelled = true;
    };
  }, [signature]);
  return map;
}
