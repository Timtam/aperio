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
