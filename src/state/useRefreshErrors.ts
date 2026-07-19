import { useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

import { getRefreshErrors } from '../api/client';
import type { AccountRefreshErrors } from '../api/types';

/**
 * Per-account refresh-error surface — the fix for SILENT staleness: a
 * container whose background refresh keeps failing (revoked iCloud
 * app-password, dead server) used to keep serving its cached rows with
 * no cue anywhere. The backend records every failed refresh in
 * `cache_sync_state.last_error` (cleared by any successful write); this
 * hook reads the aggregate and re-reads whenever a warm pass ENDS (the
 * moment errors appear or clear) plus once on mount.
 *
 * Consumers: the sidebar (per-account warning on the tree row) and the
 * accounts panel (full per-container details + the re-enter-password
 * hint for auth-shaped errors).
 */
export function useRefreshErrors(): {
  /** account_id → its failing containers. Empty map = all healthy. */
  errorsByAccount: Map<string, AccountRefreshErrors>;
} {
  const [errors, setErrors] = useState<AccountRefreshErrors[]>([]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const refetch = () => {
      getRefreshErrors()
        .then((rows) => {
          if (!cancelled) setErrors(rows);
        })
        .catch((err) => {
          console.warn('get_refresh_errors failed', err);
        });
    };

    refetch();
    listen<{ refreshing: boolean }>('cache-refresh-status', (event) => {
      // Pass END is when the error set can have changed (a pass either
      // recorded new failures or a success cleared old ones).
      if (!event.payload.refreshing) refetch();
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

  const errorsByAccount = useMemo(() => {
    const map = new Map<string, AccountRefreshErrors>();
    errors.forEach((e) => map.set(e.account_id, e));
    return map;
  }, [errors]);

  return { errorsByAccount };
}
