import { useEffect } from 'react';
import { AppState, type NativeEventSubscription } from 'react-native';

import { useAppLockLocked } from '../state/appLockContext';
import { runDeviceAccessStartCheck } from '../state/deviceAccessGate';
import { whenStartupSettled } from '../state/startupGate';

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

  useEffect(() => {
    if (locked) return;
    let cancelled = false;
    let subscription: NativeEventSubscription | null = null;
    whenStartupSettled('deviceAccess', () => {
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
    });
    return () => {
      cancelled = true;
      subscription?.remove();
    };
  }, [locked]);

  return null;
}
