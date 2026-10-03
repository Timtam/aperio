import { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo } from 'react-native';

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

import i18n from '../../i18n';
import { refreshErrors, type AccountRefreshErrors } from '../api/sync';
import {
  applyLanguageChoice,
  readLanguageChoice,
} from '../settings/language';
import {
  getCacheRefreshProgress,
  subscribeCacheRefreshProgress,
} from './cacheRefreshProgress';

/**
 * Per-account refresh-error surface — the mobile twin of the desktop
 * useRefreshErrors: a container whose background refresh keeps failing
 * (revoked provider password, dead server) used to keep serving stale
 * cached rows with no cue anywhere.
 *
 * ONE app-root watcher (started from AppContent, so it runs no matter
 * which screen the user lands on) owns the state; screens subscribe
 * through the hook and read the last published snapshot. They never
 * fetch or announce themselves, so mounting a consumer can never flash
 * a stale value or repeat the announcement.
 *
 * VISIBLE TIMING — publish the error set once the refresh has SETTLED,
 * not on every pass end. A pass end arms a short settle timer, a new
 * pass cancels it, and we publish only after `refreshing` has stayed
 * false for SETTLE_MS. This coalesces a settling round and, crucially,
 * lets a newly-mounted screen read the last settled snapshot instead of
 * a mid-refresh value — so the indicator shows a real error within
 * SETTLE_MS of the refresh finishing (no arbitrary time threshold, no
 * minutes-long wait).
 *
 * BLIP-FREE BY CONFIRMATION — the set returned by the backend is already
 * blip-filtered: a NON-auth (network) failure is only reported once it
 * has failed on two consecutive attempts (a cold-start blip's next
 * attempt succeeds and resets the count), auth-shaped failures report at
 * the first attempt, and a user-forced (manual refresh) failure reports
 * at once. So the frontend shows exactly what it is given — visible and
 * spoken are the SAME set, with no wall-clock window anywhere.
 *
 * Screen-reader-first: ONE polite app-wide announcement per newly failing
 * account — not per fetch, not per mounted screen, and never on clearing
 * (silence is the healthy state and must stay silent). Worded by the core's
 * cause (decision 181), and said again only when an account's cause grows
 * more severe (decision 185). The sentence that ends a pass already names
 * the accounts it could not update; those are not named a second time, and
 * the warnings show with that sentence instead of after the settle window
 * (decision 180, `notePassEndSpoken`).
 */

/** How long `refreshing` must stay false before the (already
 *  blip-filtered) error set is published — coalesces a settling round and
 *  lets a mounting screen read the last settled snapshot. */
const SETTLE_MS = 5_000;
const POLL_MS = 60_000;

let current: AccountRefreshErrors[] = [];
/** Per failing account, the rank it was last announced with. An account
 *  drops out when it clears, so failing again is news again. */
let announced: AnnouncedFailures = new Map();
/** Resolves once the stored language choice has been applied — the
 *  announcement must not race it and come out in the device language
 *  when the user chose another (it is deduped, so it would never repeat
 *  correctly). */
let languageSettled: Promise<unknown> = Promise.resolve();
let started = false;
let settleTimer: ReturnType<typeof setTimeout> | null = null;
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

/** Re-read the aggregate and publish it as the settled truth. Announces
 *  once if the affected-account set grew; wording comes from the NEWLY
 *  failing accounts only, so a long-known auth failure never colours an
 *  unrelated outage. */
function publishSettled(): void {
  refreshErrors()
    .then((rows) => {
      current = rows;
      listeners.forEach((l) => l(current));

      // Announce the accounts that started failing or fail worse than was
      // said. The set is already blip-filtered by the backend, so each is a
      // real, confirmed failure — no timing needed here. Wording comes from
      // those accounts only, so a long-known login problem never colours
      // an unrelated outage.
      const lead = leadingCause(toAnnounce(rows, announced));
      announced = afterAnnouncing(rows, announced);
      if (lead != null) {
        // Defer the utterance (not the decision) until the stored
        // language is live, so the one deduped announcement comes out in
        // the user's language.
        void languageSettled.then(() => {
          // Queued, never interrupting: behind a held sentence (the start
          // check's) and behind the sentence that ends a pass, which this
          // can follow at once (`notePassEndSpoken`).
          AccessibilityInfo.announceForAccessibilityWithOptions(i18n.t(announceKey(lead)), {
            queue: true,
          });
        });
      }
    })
    .catch((err) => {
      console.warn('refreshErrors failed', err);
    });
}

function announceKey(cause: RefreshCause): string {
  switch (cause) {
    case 'access':
      return 'refreshErrors.announceAccess';
    case 'auth':
      return 'refreshErrors.announceAuth';
    case 'other':
      return 'refreshErrors.announce';
  }
}

/** What a banner standing for every account says: worded by the leading
 *  cause among them (decision 181). `null` when nothing fails. */
export function refreshBannerKey(rows: readonly AccountRefreshErrors[]): string | null {
  switch (leadingCause(rows)) {
    case 'access':
      return 'refreshErrors.bannerAccess';
    case 'auth':
      return 'refreshErrors.bannerAuth';
    case 'other':
      return 'refreshErrors.banner';
    case null:
      return null;
  }
}

/**
 * The sentence that ends a pass has just named the accounts it could not
 * update (`passEndSentence`): note them as said, and show the warnings now,
 * with the sentence, instead of after the settle window (decision 180).
 */
export function notePassEndSpoken(outcome: PassOutcome): void {
  // Only what the sentence named: "nothing could be updated" names nobody,
  // and the warning with its cause still follows.
  announced = withSpoken(announced, namedByPassEnd(outcome));
  if (settleTimer != null) {
    clearTimeout(settleTimer);
    settleTimer = null;
  }
  publishSettled();
}

/** (Re)arm the settle timer — publish once refreshing has been quiet for
 *  SETTLE_MS. A new pass starting cancels it (see the subscription). */
function armSettle(): void {
  if (settleTimer != null) clearTimeout(settleTimer);
  settleTimer = setTimeout(() => {
    settleTimer = null;
    publishSettled();
  }, SETTLE_MS);
}

/**
 * Start the app-root watcher (idempotent). Called once from AppContent
 * after i18n is ready; runs for the app lifetime — the announcement must
 * fire on launch even when no consuming screen is mounted.
 */
export function startRefreshErrorsWatcher(): void {
  if (started) return;
  started = true;
  // Idempotent re-apply of the stored language: useStoredLanguage does
  // the same on mount, but the first publish below can win that race.
  languageSettled = readLanguageChoice()
    .then(applyLanguageChoice)
    .catch(() => undefined);
  subscribeCacheRefreshProgress((p) => {
    if (p.refreshing) {
      // A pass is running — hold off; wait for the burst to finish so we
      // never publish a mid-storm value that the next pass will heal.
      if (settleTimer != null) {
        clearTimeout(settleTimer);
        settleTimer = null;
      }
    } else {
      // A pass ended — arm the settle window; a new pass cancels it.
      armSettle();
    }
  });
  // If nothing is refreshing at launch, publish once the settle window
  // elapses so a persistent error carried over from last session still
  // surfaces. If a pass is in flight, its completion arms it instead.
  if (!getCacheRefreshProgress().refreshing) armSettle();
  // Safety net for the pass-less paths (per-read SWR refreshes record or
  // clear errors without a pass-end signal): when idle and not already
  // waiting to settle, re-read on a slow cadence.
  setInterval(() => {
    if (!getCacheRefreshProgress().refreshing && settleTimer == null) {
      publishSettled();
    }
  }, POLL_MS);
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
