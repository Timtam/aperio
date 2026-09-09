/**
 * Local-date key (YYYY-MM-DD) for grouping events into the day the user
 * actually sees them on.
 *
 * `Date.prototype.toISOString` returns UTC, so a Date that represents
 * "2026-05-19 00:00 local" in CEST (UTC+2) serialises as
 * "2026-05-18T22:00:00.000Z". Slicing the first ten characters would
 * then bucket the day under the wrong calendar date in any timezone
 * east of UTC — and inversely west of it. Use the local accessors
 * instead, which always reflect the user's wall-clock day.
 *
 * The grouping side and the event-start side must both go through this
 * function so they agree on which day a given timestamp belongs to.
 */
export function localDateKey(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/**
 * The inverse: a `YYYY-MM-DD` key as a local midnight, or `null` when the
 * string is not one.
 *
 * The round-trip is the whole check, and it is not pedantry.
 * `new Date(2026, 12, 99)` is not an error in JavaScript — it is April 2027 —
 * and `new Date(2026, 1, 30)` is March 2nd. A caller that only asks whether the
 * result is a valid `Date` therefore gets a confident wrong answer rather than
 * a refusal. Comparing the components back out is what separates a real day
 * from one the runtime silently repaired, and refusing beats repairing: a
 * deadline nobody typed is worse than no deadline.
 *
 * Nine other places in this package still take a key apart by hand with
 * `split('-').map(Number)` and none of them validates. They are left alone
 * here on purpose: every one of them is handed a key this app itself minted
 * with `localDateKey`, so there is nothing to reject, and converting them
 * would be a rewrite riding along with a bug fix. This is the one that takes a
 * key from stored data.
 */
export function parseDayKey(key: string): Date | null {
  const parts = key.split('-');
  if (parts.length !== 3) return null;
  const [y, m, d] = parts.map(Number);
  if (!Number.isInteger(y) || !Number.isInteger(m) || !Number.isInteger(d)) {
    return null;
  }
  const date = new Date(y, m - 1, d);
  return date.getFullYear() === y &&
    date.getMonth() === m - 1 &&
    date.getDate() === d
    ? date
    : null;
}
