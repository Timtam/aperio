import type { CacheRefreshStatus } from '../api/sync';

// App-wide external-refresh progress, published from the single app-root cache
// observer (useCacheUpdates) and read by the per-view sync indicator + the Sync
// screen. The warm pass emits a refresh_status with a growing fetched/total as
// each container completes; this exposes it without each consumer wiring its own
// native listener. Screen-reader-friendly: there is NO active announcement here
// (that would be chatter the user rejected) — the indicator's label carries the
// live "X of N", so a user hears the current progress whenever they focus it.

export interface CacheRefreshProgress {
  refreshing: boolean;
  /** Containers the running pass will refresh, or null (unknown / not running). */
  total: number | null;
  /** Containers refreshed so far, or null. */
  fetched: number | null;
}

let current: CacheRefreshProgress = { refreshing: false, total: null, fetched: null };
const listeners = new Set<(p: CacheRefreshProgress) => void>();

export function getCacheRefreshProgress(): CacheRefreshProgress {
  return current;
}

/** Publish the latest status from the native refresh_status stream. */
export function setCacheRefreshProgress(status: CacheRefreshStatus): void {
  current = {
    refreshing: status.refreshing,
    total: status.total_targets ?? null,
    fetched: status.fetched_targets ?? null,
  };
  listeners.forEach((l) => l(current));
}

export function subscribeCacheRefreshProgress(
  cb: (p: CacheRefreshProgress) => void,
): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}
