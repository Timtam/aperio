import { useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';

import {
  afterAnnouncing,
  leadingCause,
  namedByPassEnd,
  toAnnounce,
  withSpoken,
  type AnnouncedFailures,
  type PassOutcome,
  type RefreshCause,
} from '@aperio/shared';

import { getRefreshErrors } from '../api/client';
import type { AccountRefreshErrors } from '../api/types';
import { useAnnouncer } from '../a11y/announcerContext';
import { applyStoredLanguage } from '../intl/language';
import { refreshErrorAnnounceKey } from '../intl/refreshErrorKeys';

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
 * BLIP-FREE BY CONFIRMATION — the set returned by the backend is already
 * blip-filtered: a NON-auth (network) failure is only reported once it
 * has failed on two consecutive attempts (a cold-start blip's next
 * attempt succeeds and resets the count), auth-shaped failures report at
 * the first attempt, and a user-forced (manual refresh) failure reports
 * at once. So the frontend shows exactly what it is given — visible and
 * spoken are the SAME set, with no wall-clock window anywhere.
 *
 * Consumers: the sidebar (per-account warning on the tree row, and — via
 * `announceOnGrowth` — the ONE announce-on-growth instance) and the
 * accounts panel (full per-container details + the re-enter-password
 * hint).
 *
 * WORDING by the core's cause (decision 181): an account whose data the
 * system withholds leads, then a login problem, then anything else. An
 * account is announced when it starts failing and again only when its cause
 * grows more severe (decision 185). A manual refresh names the accounts it
 * could not update in its own closing sentence; those are not named a second
 * time, and the warning shows with that sentence rather than after the
 * settle window (decision 180, `notePassEndSpoken`).
 */

/** How long `refreshing` must stay false before the (already
 *  blip-filtered) error set is published — coalesces a settling round and
 *  lets a mounting consumer read the last settled snapshot. */
const SETTLE_MS = 5_000;
const POLL_MS = 60_000;

interface Publish {
  errors: AccountRefreshErrors[];
  /** What the ONE announcer says, if anything: the leading cause among the
   *  accounts that started failing or fail worse than was said. Taken from
   *  those only, so a long-known login problem never colours an unrelated
   *  outage. The set is already blip-filtered by the backend. */
  announce: RefreshCause | null;
}

let current: AccountRefreshErrors[] = [];
/** Per failing account, the rank it was last announced with; an account
 *  drops out when it clears, so failing again is news again. Updated on
 *  every settled publish whether or not anyone listens, so the decision is
 *  app-wide and made once. */
let announced: AnnouncedFailures = new Map();
let started = false;
let refreshing = false;
let settleTimer: number | null = null;
const subscribers = new Set<(p: Publish) => void>();

/** Test-only: reset the module-level singleton between tests. */
export function resetAnnouncedAccountsForTest(): void {
  announced = new Map();
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

/** Re-read the aggregate and publish it. Resolves with the cause the
 *  announcer would say; with `callerSpeaks` the subscribers are handed none,
 *  because the caller says it in its own sentence. */
function publishSettled(callerSpeaks = false): Promise<RefreshCause | null> {
  return getRefreshErrors()
    .then((rows) => {
      current = rows;
      const fresh = toAnnounce(rows, announced);
      announced = afterAnnouncing(rows, announced);
      const lead = leadingCause(fresh);
      const publish: Publish = {
        errors: current,
        announce: callerSpeaks ? null : lead,
      };
      subscribers.forEach((cb) => cb(publish));
      return lead;
    })
    .catch((err) => {
      console.warn('get_refresh_errors failed', err);
      return null;
    });
}

/**
 * A manual refresh is about to say its closing sentence (`passEndSentence`):
 * note the accounts it names as said, show the warnings now, with the
 * sentence, instead of after the settle window (decision 180), and resolve
 * with the warning's cause still to be said — for an account the sentence
 * did not name ("nothing could be updated" names nobody), or one that fails
 * worse (decision 185). The caller says both in ONE announcement: the live
 * region keeps only the last of two written in the same moment.
 */
export function notePassEndSpoken(outcome: PassOutcome): Promise<RefreshCause | null> {
  announced = withSpoken(announced, namedByPassEnd(outcome));
  if (settleTimer != null) {
    window.clearTimeout(settleTimer);
    settleTimer = null;
  }
  return publishSettled(true);
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
      const cause = p.announce;
      if (announceOnGrowth && cause != null) {
        // Defer the utterance (not the decision) until the stored
        // language is live, so the one deduped announcement comes out in
        // the user's language.
        void applyStoredLanguage()
          .catch(() => undefined)
          .then(() => {
            announceRef.current(tRef.current(refreshErrorAnnounceKey(cause)));
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
