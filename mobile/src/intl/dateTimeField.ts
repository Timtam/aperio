// String <-> Date adapters for the native @expo/ui DateTimePicker. The editor
// forms keep dates as 'YYYY-MM-DD' and times as 'HH:MM' (local, no timezone),
// matching the stored task/event shape; the picker works in Date objects. We
// read and construct in LOCAL time only — never via toISOString(), which would
// shift the day across the UTC boundary for users east/west of Greenwich.

const pad = (n: number): string => String(n).padStart(2, '0');

/** 'YYYY-MM-DD' -> a local Date at midnight. Falls back to today for an
 *  empty/invalid string, since the picker needs a non-null value. */
export function parseLocalDate(value: string): Date {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value.trim());
  if (m) return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return new Date();
}

/** A local Date -> 'YYYY-MM-DD'. */
export function formatLocalDate(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** 'HH:MM' applied onto today (the date part is irrelevant) -> a local Date.
 *  Falls back to the current time for an empty/invalid string. */
export function parseLocalTime(value: string): Date {
  const base = new Date();
  const m = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (m) base.setHours(Number(m[1]), Number(m[2]), 0, 0);
  return base;
}

/** A local Date -> 'HH:MM'. */
export function formatLocalTime(date: Date): string {
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
