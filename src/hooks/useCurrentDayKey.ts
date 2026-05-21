import { useEffect, useState } from 'react';

import { todayIsoKey } from '../intl/taskDay';

/**
 * The local `YYYY-MM-DD` for today, refreshed every minute so the
 * value updates when the date rolls over without a window reload.
 *
 * Aperio is the kind of app that gets left running for days at a
 * time (calendar + task hub on a desktop). The startup checkers
 * — CarryOver, MissedTasks, DeadlinePin — used to fire once on
 * mount and then sit silent forever, which means a user whose PC
 * runs across midnight never gets the new-day review. This hook
 * gives every consumer a stable date key that flips when the date
 * actually changes, so the same checker effect can use it as a
 * dep and re-evaluate.
 *
 * The polling interval is one minute. That's coarse enough to be
 * effectively free (one `Date()` allocation per minute) and fine
 * enough that a user-configured trigger time of, say, 08:00 fires
 * within 60 seconds of the actual moment.
 *
 * Returns a stable reference between ticks where the date hasn't
 * changed, so downstream effects don't re-run on every minute.
 */
export function useCurrentDayKey(): string {
  const [key, setKey] = useState(todayIsoKey);
  useEffect(() => {
    const interval = window.setInterval(() => {
      const next = todayIsoKey();
      setKey((prev) => (prev === next ? prev : next));
    }, 60_000);
    return () => window.clearInterval(interval);
  }, []);
  return key;
}

/**
 * The Settings → Tasks "Tageswechsel-Trigger" preference values.
 *
 *  - `'app-start'`: legacy mount-once semantics. Fires once on
 *    initial app start and never again until the next launch.
 *  - An `HH:MM` 24h string (e.g. `'00:00'`, `'08:00'`): fires once
 *    per local day, as soon as the local time on the new day has
 *    crossed the configured threshold. `'00:00'` means "as soon as
 *    the date rolls over" — the typical case.
 *
 * Storage is a plain string so future custom values fit without
 * a schema bump.
 */
export type DayStartTrigger = string;

/**
 * Decide whether a day-start checker should fire on this tick.
 *
 *  - `'app-start'` mode: fire iff we haven't fired at all yet
 *    (lastFiredDayKey is null).
 *  - HH:MM mode: fire iff we haven't fired for today AND the local
 *    clock has crossed the configured threshold.
 *
 * Pure helper so it's trivially testable.
 */
export function shouldFireToday(
  trigger: DayStartTrigger,
  lastFiredDayKey: string | null,
  todayKey: string,
  now: Date = new Date(),
): boolean {
  if (trigger === 'app-start') {
    return lastFiredDayKey === null;
  }
  if (lastFiredDayKey === todayKey) return false;
  const m = trigger.match(/^(\d{1,2}):(\d{2})$/);
  if (!m) {
    // Unparseable preference — be conservative and treat it like the
    // immediate-on-day-change default (00:00) so the user still gets
    // a review.
    return true;
  }
  const hours = Number(m[1]);
  const minutes = Number(m[2]);
  // Out-of-range numerics (e.g., "25:00") would otherwise silently
  // never trigger because `now.getHours() === 25` is impossible.
  // Treat the same as garbage — fire immediately.
  if (hours > 23 || minutes > 59) return true;
  return (
    now.getHours() > hours ||
    (now.getHours() === hours && now.getMinutes() >= minutes)
  );
}

/**
 * Persistent fire-marker for day-start checkers. Each checker stores
 * its last-fired date key in localStorage under a unique slot, so a
 * mid-day app restart doesn't re-fire the silent batch (and re-blast
 * an announcement) for a day the user already had reviewed. Loading
 * back the same key on the next mount keeps `firedRef` in sync with
 * what we already did.
 *
 * Returns `null` when storage is unavailable (private mode / quota)
 * or the value is missing — the checker treats that the same as
 * "never fired", which means the gate runs on next eligible tick.
 */
export function readFiredDayKey(slot: string): string | null {
  try {
    return localStorage.getItem(`aperio.dayStartFired.${slot}`);
  } catch {
    return null;
  }
}

export function writeFiredDayKey(slot: string, dayKey: string): void {
  try {
    localStorage.setItem(`aperio.dayStartFired.${slot}`, dayKey);
  } catch {
    // Storage unavailable; the in-memory ref still tracks the rest
    // of this session.
  }
}
