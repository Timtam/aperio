import { useEffect, useState } from 'react';

import { todayIsoKey } from '../intl/taskDay';

// `shouldFireToday` + the `DayStartTrigger` type now live in @aperio/shared
// (shared with the mobile day-start checks). Re-exported so existing desktop
// imports + tests keep their path; the hook + the localStorage fire-marker below
// stay desktop-local.
export { shouldFireToday, type DayStartTrigger } from '@aperio/shared';

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
