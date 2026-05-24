import { useCallback, useEffect, useState } from 'react';

import { listen } from '@tauri-apps/api/event';

import {
  getSyncConflictsCount,
  getSyncStatus,
  syncNow,
  type SyncStatus,
  type SyncStatusPayload,
  type SyncRoundReport,
} from '../api/client';
import { useDialogState } from './DialogState';

/**
 * Frontend bridge to the cross-device sync backend (DESIGN.md §19,
 * `src-tauri/src/event_log/`).
 *
 * Tracks two strands of state:
 *
 *   - [`SyncStatus`] — configured / in_flight / last_synced_at /
 *     interval_minutes. Polled once on mount, then refreshed
 *     reactively from `sync-status` Tauri events the scheduler
 *     emits before + after every round.
 *   - Unresolved-conflict count — drives the status-bar badge and
 *     the "Konflikte" entry in Settings. Refreshed on
 *     `sync-conflicts-changed` events the applier emits when a new
 *     conflict lands or a user-resolution sweep completes.
 *
 * Exposes a `triggerSync` for the manual button (Settings →
 * Synchronisation, or the global shortcut).
 *
 * Mounting: render once at the app shell level — the status bar
 * + Settings panel + conflicts dialog all read from the same hook
 * via React context (or by mounting an instance each, since the
 * Tauri listener model is process-wide and cheap to attach).
 *
 * ## Why not a context?
 *
 * The hook is intentionally context-free for now — each consumer
 * mounts its own instance. The listeners are per-process channels
 * so two listeners on the same Tauri event both fire and stay
 * cheap. If profiling shows the duplicate fetches matter we can
 * wrap this in a context provider later without changing the
 * consumer surface.
 */
export function useSync() {
  const { invalidateData } = useDialogState();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [lastReport, setLastReport] = useState<SyncRoundReport | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [conflictCount, setConflictCount] = useState(0);
  const [triggering, setTriggering] = useState(false);

  // Initial status pull. `getSyncStatus` is cheap (no IO), so the
  // status indicator can render the correct icon before the first
  // event arrives.
  useEffect(() => {
    let cancelled = false;
    getSyncStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('get_sync_status failed', err);
      });
    getSyncConflictsCount()
      .then((n) => {
        if (!cancelled) setConflictCount(n);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('get_sync_conflicts_count failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // `sync-status` listener — emitted before + after every round.
  // The post-round emit carries `report`; the pre-round emit
  // doesn't. We pin the latest report so the Settings panel can
  // show "12 events applied".
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<SyncStatusPayload>('sync-status', (event) => {
      const payload = event.payload;
      const { report, error, ...rest } = payload;
      setStatus(rest);
      if (report) {
        setLastReport(report);
        // A completed sync round may have applied events that
        // changed local SQLite. Bump the data version so views
        // refetch.
        if (report.applied > 0) {
          invalidateData();
        }
      }
      if (error) {
        setLastError(error);
      } else {
        // Clear the last-error on a successful round-end emit.
        if (report) setLastError(null);
      }
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('sync-status listen failed', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [invalidateData]);

  // `sync-conflicts-changed` listener — fires when the applier
  // records a new conflict + after every resolve_sync_conflict
  // call.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen('sync-conflicts-changed', () => {
      getSyncConflictsCount()
        .then((n) => setConflictCount(n))
        .catch((err) => {
          // eslint-disable-next-line no-console
          console.warn('sync conflict count refresh failed', err);
        });
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('sync-conflicts-changed listen failed', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  /** Fire a manual sync round. The backend's in-flight guard
   *  rejects parallel calls — this returns the orchestrator's
   *  error in that case so the UI can surface it. */
  const triggerSync = useCallback(async () => {
    setTriggering(true);
    setStatus((prev) => (prev ? { ...prev, in_flight: true } : prev));
    try {
      const report = await syncNow();
      setLastReport(report);
      setLastError(null);
      if (report.applied > 0) {
        invalidateData();
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('sync_now failed', err);
      setLastError(err instanceof Error ? err.message : String(err));
      setStatus((prev) =>
        prev ? { ...prev, in_flight: false } : prev,
      );
    } finally {
      setTriggering(false);
    }
  }, [invalidateData]);

  return {
    /** Latest snapshot or `null` while the initial fetch is in
     *  flight. */
    status,
    /** Counters from the most-recent completed round (manual or
     *  scheduled). `null` until the first round finishes. */
    lastReport,
    /** Last orchestrator-level error message. Cleared after the
     *  next successful round. */
    lastError,
    /** Count of unresolved conflict rows. Drives the badge. */
    conflictCount,
    /** `true` while a manual `triggerSync` call is awaiting the
     *  backend. */
    triggering,
    triggerSync,
  };
}
