import { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo } from 'react-native';

import i18n from '../../i18n';
import { refreshErrors, type AccountRefreshErrors } from '../api/sync';
import { subscribeCacheRefreshProgress } from './cacheRefreshProgress';

/**
 * Per-account refresh-error surface — the mobile twin of the desktop
 * useRefreshErrors: a container whose background refresh keeps failing
 * (revoked provider password, dead server) used to keep serving stale
 * cached rows with no cue anywhere.
 *
 * Modeled on cacheRefreshProgress: ONE app-root watcher (started from
 * AppContent, so it runs no matter which screen the user lands on)
 * fetches the aggregate on start, whenever an external warm pass ENDS,
 * and on a slow poll (per-read SWR refreshes record/clear errors
 * without a pass-end signal). Screens subscribe through the hook — they
 * never fetch or announce themselves, so mounting a consumer can never
 * repeat the announcement.
 *
 * Screen-reader-first: when the affected-account set GROWS, ONE polite
 * app-wide announcement names the failure — not per fetch, not per
 * mounted screen, and never on clearing (silence is the healthy state
 * and must stay silent).
 */

const POLL_MS = 60_000;

let current: AccountRefreshErrors[] = [];
let knownAffectedAccounts = new Set<string>();
let started = false;
const listeners = new Set<(rows: AccountRefreshErrors[]) => void>();

/**
 * Provider error text is unbounded and can embed whole HTML bodies or
 * URLs — a wall VoiceOver would read for half a minute. Collapse
 * whitespace and clamp for display; the full text stays in the log.
 */
export function clampErrorText(raw: string): string {
  const collapsed = raw.replace(/\s+/g, ' ').trim();
  return collapsed.length > 160 ? `${collapsed.slice(0, 159)}…` : collapsed;
}

function refetch(): void {
  refreshErrors()
    .then((rows) => {
      current = rows;
      const nowAffected = new Set(rows.map((r) => r.account_id));
      const grew = [...nowAffected].some(
        (id) => !knownAffectedAccounts.has(id),
      );
      if (grew) {
        AccessibilityInfo.announceForAccessibility(
          i18n.t(
            rows.some((r) => r.auth_suspected)
              ? 'refreshErrors.announceAuth'
              : 'refreshErrors.announce',
          ),
        );
      }
      knownAffectedAccounts = nowAffected;
      listeners.forEach((l) => l(current));
    })
    .catch((err) => {
      console.warn('refreshErrors failed', err);
    });
}

/**
 * Start the app-root watcher (idempotent). Called once from AppContent
 * after i18n is ready; runs for the app lifetime — the announcement
 * must fire on launch even when no consuming screen is mounted.
 */
export function startRefreshErrorsWatcher(): void {
  if (started) return;
  started = true;
  refetch();
  subscribeCacheRefreshProgress((p) => {
    if (!p.refreshing) refetch();
  });
  // Bounded staleness for the pass-less paths (per-read SWR refresh
  // failures/clears): cheap indexed query, once a minute.
  setInterval(refetch, POLL_MS);
}

export function getRefreshErrorsSnapshot(): AccountRefreshErrors[] {
  return current;
}

export function subscribeRefreshErrors(
  cb: (rows: AccountRefreshErrors[]) => void,
): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/** Thin subscriber over the app-root watcher. */
export function useRefreshErrors(): {
  /** account_id → its failing containers. Empty map = all healthy. */
  errorsByAccount: Map<string, AccountRefreshErrors>;
} {
  const [rows, setRows] = useState<AccountRefreshErrors[]>(current);

  useEffect(() => {
    setRows(current);
    return subscribeRefreshErrors(setRows);
  }, []);

  const errorsByAccount = useMemo(() => {
    const map = new Map<string, AccountRefreshErrors>();
    rows.forEach((e) => map.set(e.account_id, e));
    return map;
  }, [rows]);

  return { errorsByAccount };
}
