import { InteractionManager } from 'react-native';

/**
 * First-paint gate for the app-global startup scans (app badge, day-start
 * checks, reminder rescheduling). Their launch runs each fan out dozens of
 * FFI calls, and everything crosses ONE serial native queue — so firing
 * them at mount time made the visible screen's first read queue behind
 * minutes-of-day bookkeeping. Deferring them ~1.5s past the first settled
 * paint is imperceptible for what they do (an icon badge, a once-a-day
 * review, OS notifications scheduled days ahead) and hands the queue to
 * the screen the user is actually looking at.
 *
 * Semantics: `whenStartupSettled(key, run)` runs immediately once the gate
 * is open; before that, the LATEST run per key is parked (same-key calls
 * coalesce — each consumer has its own change-detection anyway, so one
 * post-gate run covers all pre-gate triggers). The gate opens after the
 * navigation tree has painted and interactions settled, with a hard cap so
 * a busy bridge can never starve the scans entirely.
 */

let ready = false;
const parked = new Map<string, () => void>();

export function whenStartupSettled(key: string, run: () => void): void {
  if (ready) {
    run();
    return;
  }
  parked.set(key, run);
}

/** Idempotent; flushes every parked run on first call. */
function openGate(): void {
  if (ready) return;
  ready = true;
  const runs = Array.from(parked.values());
  parked.clear();
  for (const run of runs) run();
}

/** Delay past the settled first paint before the scans start. */
const SETTLE_DELAY_MS = 1500;
/** Hard cap: open the gate this long after arming no matter what. */
const OPEN_CAP_MS = 5000;

/**
 * Arm the gate — call ONCE from the app root when the navigation tree is
 * about to mount. Returns a cleanup for symmetry with effects.
 */
export function armStartupGate(): () => void {
  const cap = setTimeout(openGate, OPEN_CAP_MS);
  const interaction = InteractionManager.runAfterInteractions(() => {
    setTimeout(openGate, SETTLE_DELAY_MS);
  });
  return () => {
    clearTimeout(cap);
    interaction.cancel();
  };
}
