import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';
import * as LocalAuthentication from 'expo-local-authentication';

import i18n from '../../i18n';

// Optional app lock — Face ID / Touch ID / device code required to open the
// app. DEVICE-LOCAL and default OFF, stored like the haptics / background-sync
// prefs: whether THIS device can authenticate biometrically is a property of
// this device (and syncing the flag to a device without any screen lock would
// lock its user out of their own calendar).
//
// This is an ACCESS lock, not encryption: the gate covers the UI and requires
// the OS's own authentication (biometrics with device-code fallback via
// expo-local-authentication); the database on disk is unchanged. Sync,
// reminders and the widget keep working behind the lock.

const KEY = 'aperio.appLock.enabled';

let cached = false;
const listeners = new Set<(enabled: boolean) => void>();

/** Whether an OS authentication sheet is up RIGHT NOW — module-level because
 *  two parties show it (the gate's unlock and the Settings toggle) and the
 *  gate must not treat either sheet's AppState flips as "the user left".
 *  A COUNTER, not a boolean: overlapping prompts (a double-tapped toggle)
 *  must not mark the world idle when the first of two sheets closes. */
let authBusy = 0;

export function isAuthenticating(): boolean {
  return authBusy > 0;
}

/** How many OS sheets that are not the lock's own are up — the calendar
 *  and reminders permission alert. A counter for the same reason as
 *  `authBusy`. */
let osSheetBusy = 0;

export function isOsSheetOpen(): boolean {
  return osSheetBusy > 0;
}

const sheetClosedListeners = new Set<() => void>();

/** Called when the last such sheet has closed. The lock gate defers its own
 *  prompt while one is up, and the 'active' that ends the sheet can arrive
 *  before the sheet's promise settles; this is the moment it may prompt. */
export function subscribeOsSheetClosed(cb: () => void): () => void {
  sheetClosedListeners.add(cb);
  return () => {
    sheetClosedListeners.delete(cb);
  };
}

/** Whether the lock is engaged right now, mirrored by AppLockGate in the same
 *  call that engages or releases it — not after a render, so a flow that runs
 *  in the same AppState event as a re-lock can see it. */
let lockEngaged = false;
const releaseWaiters = new Set<() => void>();

export function setAppLockEngaged(engaged: boolean): void {
  lockEngaged = engaged;
  if (!engaged) {
    const waiters = [...releaseWaiters];
    releaseWaiters.clear();
    waiters.forEach((release) => release());
  }
}

export function isAppLockEngaged(): boolean {
  return lockEngaged;
}

/** Resolves once the lock is not engaged: at once (`false`) when it is not,
 *  otherwise on the unlock (`true`). A sentence about the app's content waits
 *  for this rather than being spoken under the unlock prompt — and, after an
 *  unlock, a moment longer, since the closing prompt and cover would cut it
 *  off (see `AppLockGate`). */
export function whenAppLockReleased(): Promise<boolean> {
  if (!lockEngaged) return Promise.resolve(false);
  return new Promise((resolve) => {
    releaseWaiters.add(() => resolve(true));
  });
}

/**
 * Run `show` while such a sheet is up. It flips the app inactive, and a cover
 * over the app under it would flash for nothing, so the gate leaves the
 * inactive cover off and does not start the unlock prompt over it.
 *
 * Unlike the lock's own sheet, it does NOT stop the re-lock clock. An iOS
 * permission alert never backgrounds the app, so a 'background' while it is
 * up is the user really leaving, and the lock must hold on their return.
 * Android's permission dialog is an activity of its own and does background
 * the app; that counts as leaving too, because nothing tells the dialog from
 * Home or an app switch while it is up: a slow answer re-locks the app, and
 * the cover shows behind the dialog. What follows the answer — a sentence,
 * the account being added, the settings dialog — waits for the unlock
 * (`whenAppLockReleased`).
 */
export async function whileOsSheetOpen<T>(show: () => Promise<T>): Promise<T> {
  osSheetBusy += 1;
  try {
    return await show();
  } finally {
    osSheetBusy -= 1;
    if (osSheetBusy === 0) sheetClosedListeners.forEach((cb) => cb());
  }
}

/** Whether the lock COVER is on screen right now — mirrored here by
 *  AppLockGate so non-React layers (the window-level VoiceOver gesture host)
 *  can refuse to act on the content behind it. */
let coverVisible = false;

export function setAppLockCoverVisible(visible: boolean): void {
  coverVisible = visible;
}

export function isAppLockCoverVisible(): boolean {
  return coverVisible;
}

export async function readAppLockEnabled(): Promise<boolean> {
  try {
    const raw = await AsyncStorage.getItem(KEY);
    cached = raw === 'true';
  } catch {
    // Unreadable storage keeps the last known value (default: off) — failing
    // open beats locking the user out over a storage hiccup.
  }
  return cached;
}

/** Change the pref and tell every subscriber (the gate arms/disarms without a
 *  restart; the Settings switch reflects a gate-side fail-open disable). */
async function persist(next: boolean): Promise<void> {
  cached = next;
  listeners.forEach((cb) => cb(next));
  try {
    await AsyncStorage.setItem(KEY, String(next));
  } catch {
    // Best-effort; the in-memory value carries the session.
  }
}

/** Subscribe to pref changes (both directions, any writer). */
export function subscribeAppLockEnabled(cb: (enabled: boolean) => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/**
 * Run the OS authentication prompt (biometrics, falling back to the device
 * code). Three-way so callers can tell a deliberate CANCEL (the sheet itself
 * was the feedback — stay quiet) from an ERROR where possibly no sheet ever
 * appeared (say something, or the flow feels dead). Never throws.
 */
export async function authenticate(): Promise<'success' | 'cancel' | 'error'> {
  authBusy += 1;
  try {
    const result = await LocalAuthentication.authenticateAsync({
      promptMessage: i18n.t('mobile.appLock.prompt'),
      cancelLabel: i18n.t('dialogs.confirm.cancel'),
    });
    if (result.success) return 'success';
    return result.error === 'user_cancel' ||
      result.error === 'system_cancel' ||
      result.error === 'app_cancel'
      ? 'cancel'
      : 'error';
  } catch {
    return 'error';
  } finally {
    authBusy -= 1;
  }
}

/**
 * The device's enrolled protection: 'enrolled' (biometrics or a device code),
 * 'none' (nothing set up — authentication CANNOT succeed), or 'unknown' (the
 * check itself failed). The three-way answer matters: 'none' fails the lock
 * open, but a transient CHECK error must not — treating it as lost enrollment
 * would permanently disable the lock over a hiccup.
 */
export async function deviceEnrollment(): Promise<'enrolled' | 'none' | 'unknown'> {
  try {
    const level = await LocalAuthentication.getEnrolledLevelAsync();
    return level === LocalAuthentication.SecurityLevel.NONE ? 'none' : 'enrolled';
  } catch {
    return 'unknown';
  }
}

/**
 * The device's screen-lock protection disappeared while the app lock was on
 * (code removed in the OS settings). Fail OPEN and switch the pref off — a
 * calendar that bricks itself because the OS can no longer authenticate would
 * punish exactly the wrong person. Subscribers (the Settings switch) follow.
 */
export async function disableAfterLostEnrollment(): Promise<void> {
  await persist(false);
}

export type AppLockToggleResult = 'ok' | 'unavailable' | 'cancelled' | 'error';

/**
 * The Settings toggle: `[enabled, setEnabled]`, where enabling first checks
 * that the device can authenticate at all ('unavailable' otherwise) and BOTH
 * directions demand one successful authentication — the person flipping the
 * switch has to be the person who can unlock. 'cancelled' = the user closed
 * the sheet (the sheet was its own feedback); 'error' = the prompt itself
 * failed, possibly without ever appearing. One escape hatch: DISABLING on a
 * device whose enrollment is gone skips the prompt (it could never succeed,
 * and the user is already inside the app).
 */
export function useAppLockPref(): [
  boolean,
  (next: boolean) => Promise<AppLockToggleResult>,
] {
  const [enabled, setEnabled] = useState(cached);
  useEffect(() => {
    void readAppLockEnabled().then(setEnabled);
    return subscribeAppLockEnabled(setEnabled);
  }, []);
  const set = useCallback(
    async (next: boolean): Promise<AppLockToggleResult> => {
      const enrollment = await deviceEnrollment();
      if (next && enrollment === 'none') return 'unavailable';
      if (!next && enrollment === 'none') {
        await persist(false);
        return 'ok';
      }
      const auth = await authenticate();
      if (auth !== 'success') return auth === 'cancel' ? 'cancelled' : 'error';
      await persist(next);
      return 'ok';
    },
    [],
  );
  return [enabled, set];
}
