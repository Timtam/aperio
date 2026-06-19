// ── Plan-task date helpers ──────────────────────────────────────────────
// Tasks store dates as bare `YYYY-MM-DD` ISO strings (no time, no zone)
// — matching cal-core's `NaiveDate` on the wire. The helpers below
// produce strings in the user's local calendar day, which is what the
// "Heute"/"Morgen" semantics implicitly mean.
//
// Pure functions over `new Date()`, kept platform-agnostic so the desktop
// PlanTaskDialog and the mobile PlanTaskModal share one source of truth
// (and so unit tests can hit them without rendering any component).

import { localDateKey } from './dateKey';

export function isoToday(): string {
  return localDateKey(new Date());
}

export function isoTomorrow(): string {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  return localDateKey(d);
}

/**
 * "Next Monday" — matches Outlook / Apple Calendar's interpretation
 * of "next week" as "the upcoming Monday". When today *is* Monday,
 * lands one week out (next-next isn't useful as a quick preset).
 */
export function isoNextMonday(): string {
  const d = new Date();
  const dow = d.getDay(); // 0=Sun..6=Sat
  // Days until Monday: Sun→1, Mon→7, Tue→6, …, Sat→2.
  const daysToMon = dow === 0 ? 1 : (8 - dow) % 7 || 7;
  d.setDate(d.getDate() + daysToMon);
  return localDateKey(d);
}

export function formatIsoDate(iso: string): string {
  // Lightweight human-readable variant for the SR announcement; the
  // dialog itself uses the locale's date picker which is already
  // localised by the platform.
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString();
}
