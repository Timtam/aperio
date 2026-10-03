import { useEffect, useRef } from 'react';
import { AppState, type NativeEventSubscription } from 'react-native';

import { isAppLockEngaged } from '../state/appLock';
import { useAppLockLocked } from '../state/appLockContext';
import {
  runDeviceAccessForegroundCheck,
  runDeviceAccessStartCheck,
  runPendingDeviceAccessCheck,
} from '../state/deviceAccessGate';
import { whenStartupSettled } from '../state/startupGate';

/** After an unlock, how long the revealed screen gets to be read before the
 *  prompt: a flat wait, the length of the startup gate's own settle (which
 *  has long passed by then). Nothing observable says when VoiceOver is done
 *  with a screen. */
const AFTER_UNLOCK_MS = 1500;

/**
 * Where the start check for the device calendars runs (decision 166).
 *
 * Not before the app is unlocked, not before the first screen has settled (the
 * prompt must not land on a screen still being read out), and only while the
 * app is in front: a background relaunch by the OS waits for the user to bring
 * the app forward. Later, each return to the front looks whether access was
 * granted in the OS settings meanwhile. Renders nothing.
 */
export function DeviceAccessGate() {
  const locked = useAppLockLocked();
  const wasLocked = useRef(false);

  useEffect(() => {
    if (locked) {
      wasLocked.current = true;
      return;
    }
    let cancelled = false;
    let subscription: NativeEventSubscription | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const runInFront = () => {
      if (cancelled) return;
      if (AppState.currentState === 'active') {
        // A timer that fires late can land in the same 'active' as a
        // re-lock, before React has rendered it (see below).
        if (isAppLockEngaged()) return;
        void runDeviceAccessStartCheck();
        return;
      }
      subscription = AppState.addEventListener('change', (state) => {
        if (state !== 'active' || cancelled) return;
        subscription?.remove();
        subscription = null;
        // The same 'active' may re-lock the app (AppLockGate engages it in
        // this event); the alert must not race Face ID. A re-lock then runs
        // this effect again once the app is unlocked.
        if (isAppLockEngaged()) return;
        void runDeviceAccessStartCheck();
      });
    };
    whenStartupSettled('deviceAccess', () => {
      if (cancelled) return;
      if (!wasLocked.current) {
        runInFront();
        return;
      }
      // Just unlocked: VoiceOver is reading the screen the cover revealed.
      timer = setTimeout(runInFront, AFTER_UNLOCK_MS);
    });
    return () => {
      cancelled = true;
      if (timer != null) clearTimeout(timer);
      subscription?.remove();
    };
  }, [locked]);

  useEffect(() => {
    if (locked) return;
    // A return to the front the lock held back runs now, after the same
    // pause as the start check.
    const pending = setTimeout(
      runPendingDeviceAccessCheck,
      wasLocked.current ? AFTER_UNLOCK_MS : 0,
    );
    let tick: ReturnType<typeof setTimeout> | null = null;
    const subscription = AppState.addEventListener('change', (state) => {
      if (state !== 'active') return;
      // After every listener of this 'active' has run: AppLockGate may
      // engage the lock in it, and the check has to see that.
      if (tick != null) clearTimeout(tick);
      tick = setTimeout(() => void runDeviceAccessForegroundCheck(), 0);
    });
    return () => {
      clearTimeout(pending);
      if (tick != null) clearTimeout(tick);
      subscription.remove();
    };
  }, [locked]);

  return null;
}
