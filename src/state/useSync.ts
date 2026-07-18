import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { listen } from '@tauri-apps/api/event';

import {
  getSyncConflictsCount,
  getSyncStatus,
  syncNow,
  type SyncStatus,
  type SyncStatusPayload,
  type SyncRoundReport,
} from '../api/client';
import { nudgeDataReload } from './dataReloadBus';
import { useDialogState } from './dialogStateContext';
import { notify } from './notify';

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
  const { t } = useTranslation();
  const { openSyncSchemaTooOld, openSyncStaleResume } = useDialogState();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [lastReport, setLastReport] = useState<SyncRoundReport | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [conflictCount, setConflictCount] = useState(0);
  const [triggering, setTriggering] = useState(false);
  // Tracks whether we've already popped the schema-too-old modal
  // this session. We don't want to spam the user on every sync
  // round; the modal mounts once per app-launch unless the
  // backend transitions out of and back into the latched state.
  const announcedSchemaTooOldRef = useRef(false);
  // Same latch pattern for the §19.10 stale-resume dialog —
  // pops once per latch cycle so a user who dismisses it without
  // resolving doesn't get the modal again every sync round.
  const announcedStaleResumeRef = useRef(false);

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
        // changed local SQLite. Nudge the views to refetch — through
        // the CacheSyncListener coalescer, not a synchronous
        // dataVersion bump, so a round landing during a cache warm
        // pass can't bypass the reload-wave gating (the app-start
        // oscillation).
        if (report.applied > 0) {
          nudgeDataReload();
        }
        // §19.9: fire an OS-level notification when the round
        // produced field-level conflicts. The status badge
        // already shifts tone via the `sync-conflicts-changed`
        // event the scheduler emits in parallel; the
        // notification adds the "you're not currently looking
        // at Aperio" reach. Suppressed silently when the user
        // hasn't granted notification permission.
        if (report.conflicts > 0) {
          const title = t('syncStatus.notifyConflictTitle');
          const body =
            report.conflicts === 1
              ? t('syncStatus.notifyConflictBody_one')
              : t('syncStatus.notifyConflictBody_other', {
                  count: report.conflicts,
                });
          void notify(
            title,
            body,
            `notify ${report.conflicts} sync conflicts`,
          );
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
    // `t` is part of the closure for the notification message;
    // include it so a locale switch re-attaches with the new
    // translator.
  }, [t]);

  // Phase Sl: when the backend latches `schema_too_old`, pop the
  // §19.13 update modal exactly once per session. The user can
  // dismiss the modal with "Offline fortfahren"; we don't re-pop
  // unless the latched state cycles (e.g. dataset rolled back,
  // then forward again).
  useEffect(() => {
    if (status?.schema_too_old && status.min_app_version_required) {
      if (!announcedSchemaTooOldRef.current) {
        announcedSchemaTooOldRef.current = true;
        // No `running` field on the backend status; the modal
        // copy shows it from the user's perspective ("Deine
        // Version") but we don't need to thread it from here —
        // the running app version is the one rendering the
        // modal.
        openSyncSchemaTooOld(
          status.min_app_version_required,
          /* running, surfaced as the literal build number from
             the frontend; the backend already enforces the gate, so
             this string is purely informational. */ '',
        );
      }
    } else {
      // Reset the latch so a future failure-then-recovery cycle
      // can re-announce.
      announcedSchemaTooOldRef.current = false;
    }
  }, [
    status?.schema_too_old,
    status?.min_app_version_required,
    openSyncSchemaTooOld,
  ]);

  // §19.10: when the backend latches `stale_device_since`, pop
  // the resume dialog. Same once-per-cycle latch pattern as the
  // schema-too-old modal — if the user closes without resolving,
  // they can still trigger the resume via Settings later, or the
  // dialog re-pops on the next stale detection (a different
  // snapshot timestamp would also reset the latch).
  useEffect(() => {
    const since = status?.stale_device_since;
    if (since) {
      if (!announcedStaleResumeRef.current) {
        announcedStaleResumeRef.current = true;
        openSyncStaleResume(since);
      }
    } else {
      announcedStaleResumeRef.current = false;
    }
  }, [status?.stale_device_since, openSyncStaleResume]);

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
        nudgeDataReload();
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('sync_now failed', err);
      setLastError(err instanceof Error ? err.message : String(err));
    } finally {
      // Always clear `in_flight` — the sync_now command's
      // matching `sync-status` event is the source of truth,
      // but it can arrive after the awaited promise resolves
      // (the emit happens during the same backend call but
      // travels through Tauri's IPC channel separately). If
      // the listener hasn't fired yet by the time we land
      // here, this keeps the button from sitting stuck on
      // "Synchronisiert …" — the listener's later flip to the
      // same value is a no-op.
      setStatus((prev) =>
        prev ? { ...prev, in_flight: false } : prev,
      );
      setTriggering(false);
    }
  }, []);

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
