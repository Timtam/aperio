/**
 * The phone's foreground round on the one refresh interval
 * ("Aktualisierungs-Intervall", Einstellungen → Allgemein; Toni, 2026-11-09).
 *
 * Each round refreshes the external calendars, task lists and contacts, and
 * runs a device sync round when a sync target is set up. The refresh used to
 * run only after a successful sync round, so a phone without a sync target
 * refreshed its accounts at launch and on return to the app, and never while
 * it stayed open.
 *
 * The wait is read again before every sleep, so an interval changed on
 * another device applies from the next round on, and `rearm` applies one
 * changed here at once. Pure: the platform parts are handed in, so a test
 * runs it without React Native.
 */

/** What a round needs from the platform. */
export interface RoundDeps {
  /** Refresh the external data in the background (never throws). */
  refresh: () => Promise<void>;
  /** A device sync round, only when a sync target is set up (never throws). */
  syncIfConfigured: () => Promise<void>;
  /** The refresh interval in minutes, as the host resolves it. */
  intervalMinutes: () => Promise<number>;
  /** Whether the app is in the foreground: rounds pause off it. */
  isActive: () => boolean;
  setTimer: (run: () => void, ms: number) => unknown;
  clearTimer: (timer: unknown) => void;
}

/** The interval when the host cannot say. */
export const FALLBACK_INTERVAL_MINUTES = 5;

/** One round: the refresh first, without waiting for the sync round. */
export async function runRound(deps: Pick<RoundDeps, 'refresh' | 'syncIfConfigured'>): Promise<void> {
  void deps.refresh().catch(() => undefined);
  await deps.syncIfConfigured().catch(() => undefined);
}

/** The wait before the next round. */
export async function nextDelayMs(deps: Pick<RoundDeps, 'intervalMinutes'>): Promise<number> {
  let minutes = FALLBACK_INTERVAL_MINUTES;
  try {
    const read = await deps.intervalMinutes();
    if (Number.isFinite(read)) minutes = Math.max(1, read);
  } catch {
    // The host could not say: the default.
  }
  return minutes * 60_000;
}

export interface RoundLoop {
  /** Arm the next round (after a wait of one interval). */
  start: () => void;
  /** Pause: no round runs until `start`. */
  stop: () => void;
  /** The interval changed: measure the wait again from now. */
  rearm: () => void;
}

/** A timer chain: a wait of one interval, a round, the next wait. */
export function createRoundLoop(deps: RoundDeps): RoundLoop {
  let timer: unknown = null;
  // Every arm takes a new generation; a wait whose read lands after a later
  // arm (or a stop) arms nothing, so two timers never run side by side.
  let generation = 0;

  const clear = () => {
    if (timer != null) {
      deps.clearTimer(timer);
      timer = null;
    }
  };

  const arm = () => {
    clear();
    const mine = ++generation;
    void nextDelayMs(deps).then((ms) => {
      if (mine !== generation || !deps.isActive()) return;
      timer = deps.setTimer(() => {
        timer = null;
        if (mine !== generation) return;
        void runRound(deps).finally(() => {
          if (mine === generation && deps.isActive()) arm();
        });
      }, ms);
    });
  };

  return {
    start: arm,
    stop: () => {
      generation += 1;
      clear();
    },
    rearm: arm,
  };
}
