import { useEffect, useRef } from 'react';
import { AppState, InteractionManager, type NativeEventSubscription } from 'react-native';

import { useAppLockLocked } from '../state/appLockContext';
import { runDeviceAccessStartCheck } from '../state/deviceAccessGate';
import { whenStartupSettled } from '../state/startupGate';

/** After an unlock, how long the revealed screen gets to be read before the
 *  prompt — the startup gate's own settle, which has long passed by then. */
const AFTER_UNLOCK_MS = 1500;

/**
 * Where the start check for the device calendars runs (decision 166).
 *
 * Not before the app is unlocked, not before the first screen has settled (the
 * prompt must not land on a screen still being read out), and only while the
 * app is in front: a background relaunch by the OS waits for the user to bring
 * the app forward. Renders nothing.
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
        void runDeviceAccessStartCheck();
        return;
      }
      subscription = AppState.addEventListener('change', (state) => {
        if (state !== 'active' || cancelled) return;
        subscription?.remove();
        subscription = null;
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
      InteractionManager.runAfterInteractions(() => {
        timer = setTimeout(runInFront, AFTER_UNLOCK_MS);
      });
    });
    return () => {
      cancelled = true;
      if (timer != null) clearTimeout(timer);
      subscription?.remove();
    };
  }, [locked]);

  return null;
}
