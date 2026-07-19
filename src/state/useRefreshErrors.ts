import { useEffect, useMemo, useRef, useState } from 'react';
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
 * `cache_sync_state.last_error` (cleared by any successful write).
 *
 * ONE app-wide watcher (started lazily on first use, kept for the app
 * lifetime) owns the state; the sidebar and accounts panel subscribe and
 * read the last published snapshot, so a newly-mounted consumer never
 * kicks a fresh fetch that could show a mid-refresh value.
 *
 * TIMING — publish the error set only once the refresh has SETTLED, not
 * on every pass end. Cold start / a manual sync fires a BURST of warm
 * passes (an early pass can fail on a not-yet-ready network and a later
 * pass in the same burst clears it); publishing on each pass end flashed
 * the failure for a second before it healed. Instead a pass end arms a
 * short settle timer, a new pass cancels it, and we publish only after
 * `refreshing` has stayed false for SETTLE_MS — i.e. once the whole
 * round (or startup burst) is done. A genuine, persistent error survives
 * the burst and shows within SETTLE_MS of the refresh finishing (no
 * arbitrary time threshold, no minutes-long wait); a blip that healed
 * itself never shows. Auth-shaped vs. network failures differ only in
 * WORDING, never in timing.
 *
 * Consumers: the sidebar (per-account warning on the tree row, and — via
 * `announceOnGrowth` — the ONE announce-on-growth instance) and the
 * accounts panel (full per-container details + the re-enter-password
 * hint).
 */

/** How long `refreshing` must stay false before the error set is trusted
 *  and published. Long enough to bridge the gaps between a startup
 *  burst's passes, short enough that a real error still shows promptly. */
const SETTLE_MS = 5_000;
const POLL_MS = 60_000;

interface Publish {
  errors: AccountRefreshErrors[];
  /** This settled publish introduced a not-previously-known failing
   *  account (so the ONE announcer should speak). */
  grew: boolean;
  /** Wording flag: are the NEWLY failing accounts auth-shaped? A
   *  long-known auth failure must not make an unrelated outage announce
   *  as a password problem, so this is computed from the new ones only. */
  auth: boolean;
}

let current: AccountRefreshErrors[] = [];
/** Accounts already announced this session; shrinks when one clears so a
 *  re-appearing failure announces again. Updated on every settled
 *  publish regardless of whether anyone is listening, so "grew" is an
 *  app-wide-once decision. */
let knownAffectedAccounts = new Set<string>();
let started = false;
let refreshing = false;
let settleTimer: number | null = null;
const subscribers = new Set<(p: Publish) => void>();

/** Test-only: reset the module-level singleton between tests. */
export function resetAnnouncedAccountsForTest(): void {
  knownAffectedAccounts = new Set();
  current = [];
  refreshing = false;
  if (settleTimer != null) {
    clearTimeout(settleTimer);
    settleTimer = null;
  }
  started = false;
  subscribers.clear();
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

function publishSettled(): void {
  getRefreshErrors()
    .then((rows) => {
      current = rows;
      const newly = rows.filter(
        (r) => !knownAffectedAccounts.has(r.account_id),
      );
      const publish: Publish = {
        errors: current,
        grew: newly.length > 0,
        auth: newly.some((r) => r.auth_suspected),
      };
      knownAffectedAccounts = new Set(rows.map((r) => r.account_id));
      subscribers.forEach((cb) => cb(publish));
    })
    .catch((err) => {
      console.warn('get_refresh_errors failed', err);
    });
}

function armSettle(): void {
  if (settleTimer != null) window.clearTimeout(settleTimer);
  settleTimer = window.setTimeout(() => {
    settleTimer = null;
    publishSettled();
  }, SETTLE_MS);
}

function startWatcher(): void {
  if (started) return;
  started = true;
  void listen<{ refreshing: boolean }>('cache-refresh-status', (event) => {
    refreshing = event.payload.refreshing;
    if (refreshing) {
      // A pass is running — hold off; wait for the burst to finish so we
      // never publish a mid-storm value the next pass will heal.
      if (settleTimer != null) {
        window.clearTimeout(settleTimer);
        settleTimer = null;
      }
    } else {
      // A pass ended — arm the settle window; a new pass cancels it.
      armSettle();
    }
  }).catch((err) => {
    console.warn('cache-refresh-status listen failed', err);
  });
  // No synchronous "is a pass running now?" read; assume idle and arm an
  // initial settle so a persistent error from last session surfaces. If
  // a pass is actually in flight, its first event cancels this and
  // re-arms on completion.
  armSettle();
  // Safety net for the pass-less paths (per-read SWR refreshes record or
  // clear errors without a pass-end event): when idle and not already
  // waiting to settle, re-read on a slow cadence.
  window.setInterval(() => {
    if (!refreshing && settleTimer == null) publishSettled();
  }, POLL_MS);
}

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
  const [errors, setErrors] = useState<AccountRefreshErrors[]>(current);

  // Keep the latest announce/t reachable from the long-lived subscription
  // without resubscribing on every render.
  const announceRef = useRef(announce);
  announceRef.current = announce;
  const tRef = useRef(t);
  tRef.current = t;

  useEffect(() => {
    startWatcher();
    // Read the last settled snapshot immediately — no fresh fetch, so a
    // mount can never surface a mid-refresh value.
    setErrors(current);
    const cb = (p: Publish) => {
      setErrors(p.errors);
      if (announceOnGrowth && p.grew) {
        // Defer the utterance (not the decision) until the stored
        // language is live, so the one deduped announcement comes out in
        // the user's language.
        void applyStoredLanguage()
          .catch(() => undefined)
          .then(() => {
            announceRef.current(
              tRef.current(
                p.auth
                  ? 'dialogs.accounts.refreshErrors.announceAuth'
                  : 'dialogs.accounts.refreshErrors.announce',
              ),
            );
          });
      }
    };
    subscribers.add(cb);
    return () => {
      subscribers.delete(cb);
    };
  }, [announceOnGrowth]);

  const errorsByAccount = useMemo(() => {
    const map = new Map<string, AccountRefreshErrors>();
    errors.forEach((e) => map.set(e.account_id, e));
    return map;
  }, [errors]);

  return { errorsByAccount };
}
