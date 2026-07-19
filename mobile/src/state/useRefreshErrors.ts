import { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo } from 'react-native';

import i18n from '../../i18n';
import { refreshErrors, type AccountRefreshErrors } from '../api/sync';
import {
  applyLanguageChoice,
  readLanguageChoice,
} from '../settings/language';
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
/** How long a NON-auth failure must persist before it is announced —
 *  transient connectivity blips (tunnel, elevator) clear well within
 *  this and must never interrupt the user. Auth-shaped failures are
 *  announced immediately: a revoked password does not heal itself. */
const NON_AUTH_ANNOUNCE_AFTER_MS = 90_000;

let current: AccountRefreshErrors[] = [];
/** Accounts whose failure has been announced (or predates the session
 *  announce). Shrinks when an account clears, so a re-appearing failure
 *  announces again. */
let knownAffectedAccounts = new Set<string>();
/** Non-auth failures waiting out the hysteresis: account id → first
 *  seen (ms epoch). Dropped the moment the account clears. */
let pendingNonAuthSince = new Map<string, number>();
/** Resolves once the stored language choice has been applied — the
 *  launch announcement must not race it and come out in the device
 *  language when the user chose another (it is deduped, so it would
 *  never repeat correctly). */
let languageSettled: Promise<unknown> = Promise.resolve();
let started = false;
const listeners = new Set<(rows: AccountRefreshErrors[]) => void>();

/**
 * Provider error text is unbounded and can embed whole HTML bodies or
 * URLs — a wall VoiceOver would read for half a minute. Collapse
 * whitespace and clamp for display; the full text stays in the log.
 */
export function clampErrorText(raw: string): string {
  const collapsed = raw.replace(/\s+/g, ' ').trim();
  if (collapsed.length <= 160) return collapsed;
  // Cut on code points, not UTF-16 units — a split surrogate pair would
  // render (and be spoken) as a replacement character.
  return `${[...collapsed].slice(0, 159).join('')}…`;
}

function refetch(): void {
  refreshErrors()
    .then((rows) => {
      current = rows;
      const now = Date.now();
      const nowAffected = new Set(rows.map((r) => r.account_id));
      // Classify the accounts that are failing but not yet announced.
      // Auth-shaped → announce now (wording from these rows ONLY — a
      // long-known auth failure must not colour an unrelated outage).
      // Non-auth → announce only once it has persisted past the
      // hysteresis window, so connectivity blips stay silent.
      let announceAuth = false;
      let announceNonAuth = false;
      const nextPending = new Map<string, number>();
      for (const r of rows) {
        if (knownAffectedAccounts.has(r.account_id)) continue;
        if (r.auth_suspected) {
          announceAuth = true;
        } else {
          const since = pendingNonAuthSince.get(r.account_id) ?? now;
          if (now - since >= NON_AUTH_ANNOUNCE_AFTER_MS) {
            announceNonAuth = true;
          } else {
            nextPending.set(r.account_id, since);
          }
        }
      }
      if (announceAuth || announceNonAuth) {
        // Defer the utterance (not the decision) until the stored
        // language choice is live, so the one deduped announcement
        // comes out in the user's language.
        void languageSettled.then(() => {
          AccessibilityInfo.announceForAccessibility(
            i18n.t(
              announceAuth
                ? 'refreshErrors.announceAuth'
                : 'refreshErrors.announce',
            ),
          );
        });
      }
      // Known = affected minus still-pending; clearing shrinks it so a
      // re-appearing failure announces again.
      knownAffectedAccounts = new Set(
        [...nowAffected].filter((id) => !nextPending.has(id)),
      );
      pendingNonAuthSince = nextPending;
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
  // Idempotent re-apply of the stored language: useStoredLanguage does
  // the same on mount, but the first fetch below can win that race.
  languageSettled = readLanguageChoice()
    .then(applyLanguageChoice)
    .catch(() => undefined);
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
