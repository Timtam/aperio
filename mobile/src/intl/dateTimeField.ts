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

/** The day a typed time is carried on: one with no clock change, so a time
 *  the device's clock skips TODAY (02:30 on its spring-forward day) is not
 *  moved by an hour on the way to the picker and back. */
const TIME_CARRIER_DAY: readonly [number, number, number] = [2001, 0, 15];

/** 'HH:MM' on a day without a clock change (the date part is irrelevant) ->
 *  a local Date. Falls back to the current time for an empty/invalid
 *  string. */
export function parseLocalTime(value: string): Date {
  const m = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (!m) return new Date();
  const [year, month, day] = TIME_CARRIER_DAY;
  return new Date(year, month, day, Number(m[1]), Number(m[2]), 0, 0);
}

/** A local Date -> 'HH:MM'. */
export function formatLocalTime(date: Date): string {
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * The Date the Android date dialog takes for the calendar day `local` names:
 * midnight UTC of that day.
 *
 * @expo/ui hands the dialog the Date's millis unchanged, and Material3's date
 * picker reads them as a day in UTC. A local midnight east of UTC is the day
 * before in UTC, so the dialog preselected the day before, and confirming it
 * unchanged saved that day. iOS reads the device's zone and needs none of
 * this.
 */
export function toAndroidDialogDay(local: Date): Date {
  return new Date(Date.UTC(local.getFullYear(), local.getMonth(), local.getDate()));
}

/**
 * The local midnight of the day the Android date dialog returned: it answers
 * with midnight UTC of the chosen day, which west of UTC is the evening of the
 * day before on the device's clock — every chosen date read back a day early.
 */
export function fromAndroidDialogDay(utcMidnight: Date): Date {
  return new Date(
    utcMidnight.getUTCFullYear(),
    utcMidnight.getUTCMonth(),
    utcMidnight.getUTCDate(),
  );
}
