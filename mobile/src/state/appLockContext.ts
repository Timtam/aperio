import { createContext, useContext } from 'react';

// Whether the app lock currently hides the app. Provided by AppLockGate (the
// component that owns the cover) and read by the flows that AUTO-OPEN native
// modals — a React Native <Modal> lives in its own native window ABOVE any
// in-tree view, so the cover alone cannot contain it: the day-start review
// would open fully interactive on top of the locked app. Those flows hold
// while this is true and run when it flips false (the unlock).
//
// Default FALSE: a consumer rendered outside the gate (tests) behaves like an
// unlocked app.
export const AppLockLockedContext = createContext(false);

/** True while the app lock covers the app (including the pre-pref-read
 *  moment on a cold start, when it is not yet known whether a lock is on). */
export function useAppLockLocked(): boolean {
  return useContext(AppLockLockedContext);
}
