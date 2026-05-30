import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

import {
  getCacheRefreshStatus,
  refreshExternalCache,
  type CacheRefreshStatus,
  type CacheRefreshStatusPayload,
} from '../api/client';

/**
 * Frontend bridge to the backend external-cache refresher
 * (CACHE-3, `src-tauri/src/cache_refresh.rs`).
 *
 *   - Seeds from `get_cache_refresh_status` on mount so the toolbar
 *     indicator shows the persisted "last updated" before the first
 *     live event.
 *   - Subscribes to `cache-refresh-status` (emitted at the start and
 *     end of every warm pass) to flip the spinner + advance the
 *     timestamp.
 *   - Exposes `refreshNow()` for the manual "refresh" button.
 *
 * The actual data invalidation (so views pick up freshly-warmed data)
 * is handled separately by `CacheSyncListener` via `cache-updated` —
 * this hook only drives the status surface.
 */
export function useCacheRefresh() {
  const [status, setStatus] = useState<CacheRefreshStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    getCacheRefreshStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((err) => {
        console.warn('get_cache_refresh_status failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<CacheRefreshStatusPayload>('cache-refresh-status', (event) => {
      setStatus(event.payload);
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch((err) => {
        console.warn('cache-refresh-status listen failed', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const refreshNow = useCallback(async () => {
    // Optimistic spinner flip so the button reacts immediately; the
    // backend's status event overwrites this a beat later.
    setStatus((prev) =>
      prev
        ? { ...prev, refreshing: true }
        : { refreshing: true, last_refreshed_at: null },
    );
    try {
      await refreshExternalCache();
    } catch (err) {
      console.warn('refresh_external_cache failed', err);
      setStatus((prev) => (prev ? { ...prev, refreshing: false } : prev));
    }
  }, []);

  return {
    refreshing: status?.refreshing ?? false,
    lastRefreshedAt: status?.last_refreshed_at ?? null,
    refreshNow,
  };
}
