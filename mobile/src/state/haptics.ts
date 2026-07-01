import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';
import * as Haptics from 'expo-haptics';

// Haptic feedback for LOADING episodes — a view's own content load AND a
// background external-refresh pass — with a DEVICE-LOCAL on/off toggle (default
// ON). The setting is stored per device in AsyncStorage (not synced — it's a
// per-device hardware preference) and mirrored into a module-level cache so the
// (frequent) cues fire without an async read. expo-haptics exposes presets, not
// custom patterns, so the cue is a light IMPACT to begin vs. a success
// NOTIFICATION to finish.
//
// Everything routes through ONE ref-counted coordinator (hapticLoadBegin /
// hapticLoadEnd): whether the loading comes from a view load, an external
// refresh pass, or both at once (e.g. deleting an external event reloads the
// view AND kicks a background refresh), the user feels exactly ONE begin cue
// when loading starts and ONE end cue when the last source finishes — never a
// double-buzz. The begin cue is DELAYED (LOAD_HAPTIC_DELAY_MS): a load that
// finishes faster than the delay stays silent, so quick navigation between warm
// views doesn't buzz — only a perceptibly slow load does.

const KEY = 'aperio.haptics.enabled';
let cached = true;

/** Load the stored pref into the cache. Call once on app start. */
export async function loadHapticsPref(): Promise<void> {
  try {
    const v = await AsyncStorage.getItem(KEY);
    if (v != null) cached = v === 'true';
  } catch {
    // Best-effort; the default (on) stays.
  }
}

async function persist(enabled: boolean): Promise<void> {
  cached = enabled;
  try {
    await AsyncStorage.setItem(KEY, String(enabled));
  } catch {
    // Best-effort.
  }
}

/** Settings hook: the current value + a setter that persists + updates the
 *  cache so the cues honour it immediately. */
export function useHapticsPref(): [boolean, (next: boolean) => void] {
  const [enabled, setEnabled] = useState(cached);
  useEffect(() => {
    void AsyncStorage.getItem(KEY).then((v) => {
      if (v != null) setEnabled(v === 'true');
    });
  }, []);
  const set = useCallback((next: boolean) => {
    setEnabled(next);
    void persist(next);
  }, []);
  return [enabled, set];
}

// ── Loading-cue coordinator ────────────────────────────────────────────────
// Only buzz once loading is slow enough to notice, and only once per episode no
// matter how many concurrent sources (view loads + refresh passes) overlap.

/** How long a load must run before the begin cue fires — instant/warm loads
 *  stay silent, only a perceptibly slow one buzzes. */
const LOAD_HAPTIC_DELAY_MS = 200;

/** Safety net: force-close an episode that never got its closing end. The
 *  view-side (finally) always balances, but the external-refresh pass
 *  (cacheObserver) trusts the native Host to emit a closing `refreshing:false`
 *  edge; if that edge is ever dropped (a pass aborted / torn down mid-refresh),
 *  the ref count would leak and suppress the cue for the rest of the session.
 *  This watchdog resets a stuck episode. Comfortably longer than any real
 *  foreground load/refresh so it never truncates a legitimate one. */
const EPISODE_MAX_MS = 60_000;

let activeLoads = 0;
let startTimer: ReturnType<typeof setTimeout> | null = null;
let episodeTimer: ReturnType<typeof setTimeout> | null = null;
let beganCue = false; // did we actually fire the begin cue (the delay elapsed)?

/** Close the current episode: clear both timers, reset the ref count, and fire
 *  the success cue iff the begin cue had fired (so a genuinely-slow load still
 *  gets a matched bracket, and a sub-delay load gets neither). */
function closeEpisode(): void {
  if (startTimer != null) {
    clearTimeout(startTimer);
    startTimer = null;
  }
  if (episodeTimer != null) {
    clearTimeout(episodeTimer);
    episodeTimer = null;
  }
  const buzz = beganCue && cached;
  activeLoads = 0;
  beganCue = false;
  if (buzz) {
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(
      () => undefined,
    );
  }
}

/** Signal that a loading episode began (a view load or an external refresh
 *  pass). Ref-counted + debounced: the light begin cue fires once, only if the
 *  first load is still running after `LOAD_HAPTIC_DELAY_MS`. Pair EVERY call
 *  with exactly one `hapticLoadEnd` (e.g. in a `finally`). No-op-ish when the
 *  pref is off (still ref-counts, just never buzzes). */
export function hapticLoadBegin(): void {
  activeLoads += 1;
  // Only the load that opens a fresh episode (0→1) arms the timers; overlapping
  // loads just bump the count.
  if (activeLoads !== 1) return;
  if (startTimer == null && !beganCue) {
    startTimer = setTimeout(() => {
      startTimer = null;
      if (activeLoads > 0) {
        beganCue = true;
        if (cached) {
          void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(
            () => undefined,
          );
        }
      }
    }, LOAD_HAPTIC_DELAY_MS);
  }
  // Arm the stuck-episode watchdog once per episode.
  if (episodeTimer == null) {
    episodeTimer = setTimeout(closeEpisode, EPISODE_MAX_MS);
  }
}

/** Signal that a loading episode finished. When the last concurrent load ends,
 *  fires the success cue IFF the begin cue fired (a load shorter than the delay
 *  gets neither cue). Safe to over-call — it floors at zero. */
export function hapticLoadEnd(): void {
  if (activeLoads === 0) return;
  activeLoads -= 1;
  if (activeLoads > 0) return;
  closeEpisode();
}
