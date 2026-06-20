// JS-side sync-activity tracking so the status indicator can show "uploading"
// the instant a round starts.
//
// The engine sets `in_flight` for a round's duration, but the status indicator
// only POLLS the engine every 30s — far too coarse to land inside a
// seconds-long round, so it would otherwise read "synced" throughout an active
// sync. Mobile has no scheduler: every round is kicked from JS (syncNow + the
// onboarding/resume paths), so wrapping those calls in `withSyncActivity`
// captures all foreground sync activity reliably and immediately.
//
// Ref-counted so overlapping rounds (e.g. a foreground round during a manual
// one) don't end activity early. Listeners fire on every 0↔n transition.

type Listener = (active: boolean) => void;

let depth = 0;
const listeners = new Set<Listener>();

/** True while at least one tracked sync round is running. */
export function isSyncing(): boolean {
  return depth > 0;
}

/** Subscribe to activity transitions (`true` on start, `false` when the last
 *  round ends); returns an unsubscribe fn. */
export function subscribeSyncActivity(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function emit(): void {
  const active = depth > 0;
  listeners.forEach((l) => l(active));
}

/** Run `op` as a tracked sync round: activity is on for its duration (even if it
 *  rejects), then off — so the indicator shows "uploading" while it runs and
 *  settles to the real status afterwards. */
export async function withSyncActivity<T>(op: () => Promise<T>): Promise<T> {
  depth += 1;
  if (depth === 1) emit();
  try {
    return await op();
  } finally {
    depth -= 1;
    if (depth === 0) emit();
  }
}
