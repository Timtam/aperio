import { cacheRefreshStatus, warmCacheOnForeground } from '../api/sync';

// Shared "wait for the external caches to become readable" primitive, used by
// the day-start checks (useDayStartChecks) and the reminder scheduler's launch
// pass. Extracted because both consumers need the SAME answer to the same
// question — "can a cache-only read see today's data yet?" — and the
// device-reminders account is the recurring victim of asking too early: its
// bridge installs right after the Host opens, so a cold read enumerates its
// tasks as empty/missing.

/** Cap on waiting for the external warm pass before the caller proceeds
 *  anyway. Offline (no pass, or a failing one) must not block forever —
 *  and offline, blocking live reads degraded to empty too, so running
 *  local-only there is parity. */
const CACHE_SETTLE_CAP_MS = 60_000;
/** How long "not refreshing" still means "the pass has not STARTED yet".
 *  The warm kick returns before the pass does anything, so an immediate
 *  status read says `refreshing: false` — indistinguishable from
 *  "finished". After the grace, "not refreshing" is taken at face value,
 *  which is also the honest answer on a device with no external accounts.
 *  (Same reasoning, constants and shape as backgroundSync's
 *  waitForExternalRefresh.) */
const WARM_START_GRACE_MS = 2_000;

/** Poll cadence for the settle — a bridge read per half-second for at most a
 *  minute, only on the passes that actually need settled data. */
const CACHE_SETTLE_POLL_MS = 500;

/**
 * Kick an (unforced) external warm pass and wait for it to finish, reporting
 * whether the finish was CONFIRMED or the wait gave up ('capped').
 *
 * The day-start checks burn once-a-day fire-markers; with the read path
 * cache-only, evaluating against a cold external cache would burn the
 * markers against EMPTY data — silently dropping the day's deadline-pin,
 * carry-over, review and spoken reminders for every external task. The
 * device-reminders account was the visible victim: its bridge installs
 * right after the Host opens, so the launch warm pass can enumerate its
 * targets BEFORE that account exists. Kicking our OWN pass here (unforced —
 * fresh containers cost nothing) guarantees every registered account has
 * been offered one refresh before anything is decided.
 *
 * The wait POLLS the native status (`cacheRefreshStatus`, the proven shape
 * of backgroundSync's waitForExternalRefresh) instead of trusting the JS
 * push mirror: at a cold start — and opening the app from the day-start
 * notification is exactly that — the first refresh_status push can arrive
 * seconds late, and a push that fired while JS was suspended leaves the
 * mirror stale in either direction. The bridge read asks the Host directly.
 */
export async function settleExternalCaches(): Promise<'confirmed' | 'capped'> {
  try {
    await warmCacheOnForeground();
  } catch {
    // The kick itself failed — no pass will ever report back.
    return 'capped';
  }
  const deadline = Date.now() + CACHE_SETTLE_CAP_MS;
  const startedBy = Date.now() + WARM_START_GRACE_MS;
  let seenRunning = false;
  while (Date.now() < deadline) {
    let refreshing: boolean;
    try {
      refreshing = (await cacheRefreshStatus()).refreshing;
    } catch {
      return 'capped';
    }
    if (refreshing) {
      seenRunning = true;
    } else if (seenRunning || Date.now() > startedBy) {
      // Ran and finished — or never started within the grace, which the
      // native status makes trustworthy: nothing needed refreshing (all
      // fresh / no external accounts), so the caches are as warm as they get.
      return 'confirmed';
    }
    await new Promise((resolve) => setTimeout(resolve, CACHE_SETTLE_POLL_MS));
  }
  return 'capped';
}
