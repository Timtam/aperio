// Day-start review flow.
//
// The pure selectors — `filterOverdue` (deadline lapsed), `filterCarriedOver`
// (scheduled day lapsed, cascade-aware), `actionableDescendants` — now live in
// `@aperio/shared` so the desktop checker and the mobile day-start checks share
// one implementation. They're re-exported here so existing desktop imports +
// tests keep their path. The snooze plumbing below stays desktop-local (it's
// localStorage-backed; the mobile checks use their own AsyncStorage marker).
export {
  filterOverdue,
  filterCarriedOver,
  actionableDescendants,
} from '@aperio/shared';

// ── Snooze plumbing ────────────────────────────────────────────────────

const SNOOZE_KEY = 'aperio.dayStartReview.snoozeUntil';

/** Legacy keys from the two-dialog era. We still respect them on read so a
 *  snooze set on the old build doesn't suddenly stop being effective after the
 *  upgrade — the 4-hour window expires by itself and writes from then on use the
 *  unified key. */
const LEGACY_MISSED_KEY = 'aperio.missedTasks.snoozeUntil';
const LEGACY_CARRY_OVER_KEY = 'aperio.carryOver.snoozeUntil';

/** Suppress the unified review for `hours` hours. */
export function snoozeDayStartReview(hours: number): void {
  try {
    const until = Date.now() + hours * 60 * 60 * 1000;
    localStorage.setItem(SNOOZE_KEY, String(until));
  } catch {
    // Storage unavailable; the dialog simply re-appears on the next tick / start.
  }
}

export function isDayStartReviewSnoozed(): boolean {
  try {
    for (const key of [SNOOZE_KEY, LEGACY_MISSED_KEY, LEGACY_CARRY_OVER_KEY]) {
      const raw = localStorage.getItem(key);
      if (!raw) continue;
      const until = Number.parseInt(raw, 10);
      if (Number.isNaN(until)) continue;
      if (Date.now() < until) return true;
    }
    return false;
  } catch {
    return false;
  }
}
