import AsyncStorage from '@react-native-async-storage/async-storage';

// Snooze plumbing for the day-start review — the mobile twin of the desktop's
// localStorage snooze in dayStartReview.ts, backed by AsyncStorage instead.
// Closing the review (handled-everything / "remind me later" / hardware back)
// suppresses the whole gate for four hours. Device-local (NOT synced): each
// device runs its own day-start checks. Best-effort: a storage failure reads as
// "not snoozed" so a real future trigger is never silently lost.
//
// There's no legacy two-dialog era on mobile (the review shipped unified), so —
// unlike the desktop — there are no legacy keys to honour on read.

const SNOOZE_KEY = 'aperio.dayStartReview.snoozeUntil';

/** Suppress the review for `hours` hours. */
export async function snoozeDayStartReview(hours: number): Promise<void> {
  try {
    const until = Date.now() + hours * 60 * 60 * 1000;
    await AsyncStorage.setItem(SNOOZE_KEY, String(until));
  } catch {
    // Storage unavailable; the review simply re-appears on the next eligible tick.
  }
}

export async function isDayStartReviewSnoozed(): Promise<boolean> {
  try {
    const raw = await AsyncStorage.getItem(SNOOZE_KEY);
    if (!raw) return false;
    const until = Number.parseInt(raw, 10);
    if (Number.isNaN(until)) return false;
    return Date.now() < until;
  } catch {
    return false;
  }
}
