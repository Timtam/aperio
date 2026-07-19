import { useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';

import { getRefreshErrors } from '../api/client';
import type { AccountRefreshErrors } from '../api/types';
import { useAnnouncer } from '../a11y/announcerContext';
import { applyStoredLanguage } from '../intl/language';

/**
 * Per-account refresh-error surface — the fix for SILENT staleness: a
 * container whose background refresh keeps failing (revoked iCloud
 * app-password, dead server) used to keep serving its cached rows with
 * no cue anywhere. The backend records every failed refresh in
 * `cache_sync_state.last_error` (cleared by any successful write); this
 * hook reads the aggregate and re-reads whenever a warm pass ENDS (the
 * moment errors appear or clear) plus once on mount, plus a slow poll —
 * per-read SWR refreshes record/clear errors without a pass-end event,
 * so without the poll a just-fixed password would keep the warning (and
 * a fresh failure stay invisible) until the next scheduled pass.
 *
 * Consumers: the sidebar (per-account warning on the tree row, and the
 * ONE announce-on-growth instance) and the accounts panel (full
 * per-container details + the re-enter-password hint).
 */

/**
 * App-wide announce dedup, deliberately at module scope: several hook
 * instances may be live (sidebar + accounts panel), and each remount
 * starts a fresh effect — but a pre-existing error must be announced
 * exactly once per session, not once per instance or per mount.
 * Shrinks when an account clears, so a re-appearing failure announces
 * again.
 */
let knownAffectedAccounts = new Set<string>();
/** Non-auth failures waiting out the announce hysteresis: account id →
 *  first seen (ms epoch). Dropped the moment the account clears. */
let pendingNonAuthSince = new Map<string, number>();

/** How long a NON-auth failure must persist before it is announced —
 *  connectivity blips (sleep/wake, Wi-Fi drop) clear well within this
 *  and must never interrupt the user. Auth-shaped failures announce
 *  immediately: a revoked password does not heal itself. */
const NON_AUTH_ANNOUNCE_AFTER_MS = 90_000;

/** Test-only: reset the module-level announce dedup between tests. */
export function resetAnnouncedAccountsForTest(): void {
  knownAffectedAccounts = new Set();
  pendingNonAuthSince = new Map();
}

/**
 * Provider error text is unbounded and can embed whole HTML bodies or
 * URLs — a wall NVDA would read for half a minute. Collapse whitespace
 * and clamp for display; the full text stays in the log/backend.
 */
export function clampErrorText(raw: string): string {
  const collapsed = raw.replace(/\s+/g, ' ').trim();
  if (collapsed.length <= 160) return collapsed;
  // Cut on code points, not UTF-16 units — a split surrogate pair would
  // render (and be spoken) as a replacement character.
  return `${[...collapsed].slice(0, 159).join('')}…`;
}

const POLL_MS = 60_000;

export function useRefreshErrors(options?: {
  /**
   * Announce (politely) when a NEW account starts failing. Pass `true`
   * from exactly ONE always-mounted consumer (the sidebar) so a blind
   * user learns about the failure without having to stumble onto the
   * account row; every other consumer stays silent.
   */
  announceOnGrowth?: boolean;
}): {
  /** account_id → its failing containers. Empty map = all healthy. */
  errorsByAccount: Map<string, AccountRefreshErrors>;
} {
  const announceOnGrowth = options?.announceOnGrowth === true;
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const [errors, setErrors] = useState<AccountRefreshErrors[]>([]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const refetch = () => {
      getRefreshErrors()
        .then((rows) => {
          if (cancelled) return;
          setErrors(rows);
          const nowAffected = new Set(rows.map((r) => r.account_id));
          if (announceOnGrowth) {
            // Classify the accounts that are failing but not yet
            // announced. Wording comes from the NEWLY failing accounts
            // only — a long-known auth failure must not make an
            // unrelated outage announce as a password problem. Auth →
            // announce now; non-auth only after the hysteresis window,
            // so connectivity blips stay silent.
            const now = Date.now();
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
              // language choice is live — the one deduped announcement
              // must come out in the user's language, and the first
              // fetch can win the race against useStoredLanguage.
              void applyStoredLanguage()
                .catch(() => undefined)
                .then(() => {
                  announce(
                    t(
                      announceAuth
                        ? 'dialogs.accounts.refreshErrors.announceAuth'
                        : 'dialogs.accounts.refreshErrors.announce',
                    ),
                  );
                });
            }
            knownAffectedAccounts = new Set(
              [...nowAffected].filter((id) => !nextPending.has(id)),
            );
            pendingNonAuthSince = nextPending;
          }
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
    // Bounded staleness for the pass-less paths (per-read SWR refresh
    // failures/clears): cheap indexed query, once a minute.
    const poll = window.setInterval(refetch, POLL_MS);

    return () => {
      cancelled = true;
      unlisten?.();
      window.clearInterval(poll);
    };
  }, [announceOnGrowth, announce, t]);

  const errorsByAccount = useMemo(() => {
    const map = new Map<string, AccountRefreshErrors>();
    errors.forEach((e) => map.set(e.account_id, e));
    return map;
  }, [errors]);

  return { errorsByAccount };
}
