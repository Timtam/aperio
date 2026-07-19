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
 * VISIBLE TIMING — publish the error set once the refresh has SETTLED,
 * not on every pass end. A pass end arms a short settle timer, a new
 * pass cancels it, and we publish only after `refreshing` has stayed
 * false for SETTLE_MS. This coalesces a settling round and lets a
 * newly-mounted consumer read the last settled snapshot instead of a
 * mid-refresh value — the warning shows within SETTLE_MS of the refresh
 * finishing (no arbitrary time threshold, no minutes-long wait).
 *
 * ANNOUNCE TIMING — the spoken alarm is stricter than the visible cue,
 * because a launch-time connectivity blip must never falsely interrupt a
 * blind user. Cold start is a SINGLE warm pass; if the network is not up
 * yet it fails, and the error only heals later via an out-of-band SWR
 * read (which emits no refresh-status signal), so the settle gate alone
 * cannot tell that blip from a real outage. So: auth-shaped failures
 * (revoked password — never self-heals) announce on the first settled
 * publish; NON-auth (network) failures announce only after they have
 * PERSISTED past NON_AUTH_ANNOUNCE_AFTER_MS, re-evaluated by the poll.
 * The VISIBLE surface is unaffected by this window; only the utterance
 * waits.
 *
 * Consumers: the sidebar (per-account warning on the tree row, and — via
 * `announceOnGrowth` — the ONE announce-on-growth instance) and the
 * accounts panel (full per-container details + the re-enter-password
 * hint).
 */

/** How long `refreshing` must stay false before the error set is trusted
 *  and published to the VISIBLE surface. */
const SETTLE_MS = 5_000;
/** How long a NON-auth failure must persist before it is ANNOUNCED — a
 *  launch/connectivity blip clears well within this and must never
 *  interrupt the user. Auth-shaped failures announce immediately. */
const NON_AUTH_ANNOUNCE_AFTER_MS = 90_000;
const POLL_MS = 60_000;

interface Publish {
  errors: AccountRefreshErrors[];
  /** This settled publish has a newly-failing account that is due to be
   *  ANNOUNCED now (auth-shaped, or non-auth past its persistence
   *  window) — so the ONE announcer should speak. */
  announce: boolean;
  /** Wording flag: is the announcement driven by an auth-shaped failure?
   *  A long-known auth failure must not make an unrelated outage announce
   *  as a password problem, so this is computed from the new ones only. */
  auth: boolean;
}

let current: AccountRefreshErrors[] = [];
/** Accounts already announced this session; shrinks when one clears so a
 *  re-appearing failure announces again. Updated on every settled
 *  publish regardless of whether anyone is listening, so the announce
 *  decision is app-wide-once. A non-auth account is NOT added here until
 *  it out-persists the window, so it stays eligible to announce later. */
let knownAffectedAccounts = new Set<string>();
/** Non-auth failures waiting out the announce window: account id → first
 *  seen (ms epoch). Dropped the moment the account clears. */
let pendingNonAuthSince = new Map<string, number>();
let started = false;
let refreshing = false;
let settleTimer: number | null = null;
const subscribers = new Set<(p: Publish) => void>();

/** Test-only: reset the module-level singleton between tests. */
export function resetAnnouncedAccountsForTest(): void {
  knownAffectedAccounts = new Set();
  pendingNonAuthSince = new Map();
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
      // ANNOUNCE decision: auth-shaped newly-failing accounts speak now;
      // non-auth only once they out-persist the window (the poll
      // re-evaluates). An account still pending is NOT yet "known", so it
      // stays eligible. The VISIBLE surface (publish.errors) is always
      // the settled snapshot, independent of this window.
      const now = Date.now();
      const nowAffected = new Set(rows.map((r) => r.account_id));
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
      knownAffectedAccounts = new Set(
        [...nowAffected].filter((id) => !nextPending.has(id)),
      );
      pendingNonAuthSince = nextPending;
      const publish: Publish = {
        errors: current,
        announce: announceAuth || announceNonAuth,
        auth: announceAuth,
      };
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
      if (announceOnGrowth && p.announce) {
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
