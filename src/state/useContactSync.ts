import { useCallback, useEffect, useState } from 'react';

import { listen } from '@tauri-apps/api/event';

import {
  getContactsSyncStatus,
  syncContactsNow,
  type ContactsSyncStatus,
  type ContactsSyncedPayload,
} from '../api/client';
import { useDialogState } from './DialogState';

/**
 * Frontend bridge to the backend contact sync scheduler
 * (DESIGN.md §10.5, backend `crates/src-tauri/src/contact_sync.rs`).
 *
 * Responsibilities:
 *
 *   - Subscribe to the `contacts-synced` Tauri event so the panel
 *     footer reflects the latest sync time without polling.
 *   - When a sync completes, bump the global `dataVersion` so
 *     `useContacts` (and any other contact-aware hook) drops its
 *     SWR cache and refetches via `get_contacts` — that's how
 *     the freshly-warmed adapter cache reaches the UI.
 *   - Expose `triggerSync(includeReadOnly?)` so the "Refresh"
 *     button can drive a manual pass.
 *
 * Mounting: render once at the top of the contacts view; the
 * event listener and initial status fetch run on mount and clean
 * up on unmount. The hook is intentionally cheap when the view
 * isn't visible — no contacts pages, no listener attached.
 */
export function useContactSync() {
  const { invalidateData } = useDialogState();
  const [status, setStatus] = useState<ContactsSyncStatus | null>(null);
  const [triggering, setTriggering] = useState(false);

  // Initial pull so the footer renders the persisted timestamp
  // before the first event arrives. The backend hydrates the
  // status from `user_prefs.contacts.lastSyncedAt` so this works
  // after a restart too.
  useEffect(() => {
    let cancelled = false;
    getContactsSyncStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('get_contacts_sync_status failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe to the per-pass completion event. `listen` returns
  // a promise that resolves to an unlisten function; the cleanup
  // awaits it through a closed-over variable so the effect can
  // dispose synchronously.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<ContactsSyncedPayload>('contacts-synced', (event) => {
      const payload = event.payload;
      setStatus((prev) => ({
        last_synced_at: payload.last_synced_at,
        // Preserve the configured interval (the event payload
        // doesn't carry it) and assume the pass is done by the
        // time we receive the event.
        interval_minutes: prev?.interval_minutes ?? 60,
        in_flight: false,
        // Same preserve-or-default pattern for the new pref —
        // the event doesn't carry it either; ContactsPanel
        // re-syncs against the next status fetch.
        include_read_only_on_sync:
          prev?.include_read_only_on_sync ?? false,
      }));
      // Bump the global data version so `useContacts` drops its
      // SWR cache. Adapter-side caches are now warm, so the
      // refetch is cheap.
      invalidateData();
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
        console.warn('contacts-synced listen failed', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [invalidateData]);

  /** Fire a manual sync pass. `includeReadOnly` is an explicit
   *  override: `true` / `false` force the choice for this one
   *  call, `undefined` defers to the persisted
   *  `contacts.includeReadOnlyOnSync` pref so manual + periodic
   *  passes match. Most call sites should omit the argument. */
  const triggerSync = useCallback(async (includeReadOnly?: boolean) => {
    setTriggering(true);
    // Optimistic in_flight flip so the spinner appears
    // immediately without waiting for the next status fetch.
    setStatus((prev) =>
      prev ? { ...prev, in_flight: true } : prev,
    );
    try {
      const ran = await syncContactsNow(includeReadOnly);
      if (!ran) {
        // Another pass beat us to the in-flight guard. Pull the
        // status so the spinner reflects the real state.
        const fresh = await getContactsSyncStatus();
        setStatus(fresh);
      }
      // On success the `contacts-synced` listener will fire and
      // overwrite the status — nothing further to do here.
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('sync_contacts_now failed', err);
      // Roll the in_flight flag back so the user can retry.
      setStatus((prev) =>
        prev ? { ...prev, in_flight: false } : prev,
      );
    } finally {
      setTriggering(false);
    }
  }, []);

  return {
    /** Latest sync status snapshot, or `null` while the initial
     *  fetch is still in flight. */
    status,
    /** True while a manual `triggerSync` call is awaiting the
     *  backend reply. Distinct from `status.in_flight` so the UI
     *  can debounce double-clicks even if the backend pass is
     *  already running from a parallel trigger. */
    triggering,
    triggerSync,
  };
}
