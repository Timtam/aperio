import { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo } from 'react-native';

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
 * TIMING — surface the error set only once the refresh has SETTLED, not
 * on every pass end. Cold start fires a BURST of warm passes (network
 * not ready → an early pass fails → a later pass in the same burst
 * clears it); publishing on each pass end flashed the failure for a
 * second before it healed. Instead a pass end arms a short settle timer,
 * a new pass cancels it, and we publish only after `refreshing` has
 * stayed false for SETTLE_MS — i.e. once the whole round (or startup
 * burst) is done. A genuine, persistent error survives the burst and
 * shows within SETTLE_MS of the refresh finishing (no minutes-long
 * wait, no arbitrary time threshold); a blip that healed itself never
 * shows. Auth-shaped vs. network failures differ only in WORDING, never
 * in timing.
 *
 * Screen-reader-first: when the affected-account set GROWS on a settled
 * publish, ONE polite app-wide announcement names the failure — not per
 * fetch, not per mounted screen, and never on clearing (silence is the
 * healthy state and must stay silent).
 */

/** How long `refreshing` must stay false before the error set is trusted
 *  and published. Long enough to bridge the gaps between a cold-start
 *  burst's passes, short enough that a real error still shows promptly
 *  after the refresh finishes. */
const SETTLE_MS = 5_000;
const POLL_MS = 60_000;

let current: AccountRefreshErrors[] = [];
/** Accounts whose failure has already been announced this session.
 *  Shrinks when an account clears, so a re-appearing failure announces
 *  again. */
let knownAffectedAccounts = new Set<string>();
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
      const nowAffected = new Set(rows.map((r) => r.account_id));
      const newly = rows.filter((r) => !knownAffectedAccounts.has(r.account_id));
      if (newly.length > 0) {
        const auth = newly.some((r) => r.auth_suspected);
        // Defer the utterance (not the decision) until the stored
        // language is live, so the one deduped announcement comes out in
        // the user's language.
        void languageSettled.then(() => {
          AccessibilityInfo.announceForAccessibility(
            i18n.t(
              auth ? 'refreshErrors.announceAuth' : 'refreshErrors.announce',
            ),
          );
        });
      }
      knownAffectedAccounts = nowAffected;
      listeners.forEach((l) => l(current));
    })
    .catch((err) => {
      console.warn('refreshErrors failed', err);
    });
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
