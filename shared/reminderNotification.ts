/**
 * The number of whole calendar days an all-day event occurrence spans, given its
 * RFC-3339 `start` and its EXCLUSIVE `end` (the next-midnight boundary an all-day
 * event stores). `1` for a single-day all-day event, `> 1` for a multi-day one.
 *
 * Works on the millisecond delta, so it's robust to the local-midnight-as-UTC
 * anchoring all-day events use. Degrades to `1` (the single-day label) for an
 * unparseable or non-positive span. Used by the reminder scheduler to choose
 * between a plain "all day" body and an "all day · <from> to <to>" range.
 */
export function allDayReminderDays(startIso: string, endIso: string): number {
  const start = new Date(startIso).getTime();
  const end = new Date(endIso).getTime();
  if (!Number.isFinite(start) || !Number.isFinite(end)) return 1;
  const DAY_MS = 86_400_000;
  const days = Math.round((end - start) / DAY_MS);
  return days >= 1 ? days : 1;
}
