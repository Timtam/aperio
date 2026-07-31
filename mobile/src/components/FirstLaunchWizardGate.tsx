import { useEffect, useRef, useState } from 'react';

import { isFreshInstance } from '../api/onboarding';
import { getUserPref, setUserPref } from '../api/prefs';
import { FirstLaunchWizardModal } from './FirstLaunchWizardModal';

/** Marker that the first-launch check has already run, so an empty instance
 *  that dismissed (or was never offered) the wizard isn't re-evaluated on every
 *  launch. Shares its key with the desktop checker.
 *
 *  In `user_prefs`, inside the data directory, rather than in AsyncStorage.
 *  Still device-local, because `onboarding.` is absent from the sync whitelist
 *  (`crates/sync-engine/src/whitelist.rs`) — do not add it there. */
const EVALUATED_KEY = 'onboarding.firstLaunchWizardEvaluated';

/**
 * Startup gate for the first-launch wizard (DESIGN.md §19.11), the mobile twin
 * of the desktop `FirstLaunchWizardChecker`. On the first launch of a genuinely
 * FRESH instance — no external account, no sync target, an empty local store
 * (the `is_fresh_instance` composition) — it opens the wizard once, paired with
 * the marker above so an established (or already-dismissed) install is never
 * prompted.
 */
export function FirstLaunchWizardGate() {
  const [open, setOpen] = useState(false);
  const evaluatedRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        if ((await getUserPref(EVALUATED_KEY)) === 'true') return;
        const fresh = await isFreshInstance();
        if (cancelled) return;
        evaluatedRef.current = true;
        // A failed write costs one redundant evaluation next launch. It must
        // not cost the user the wizard, so it cannot sit between the answer and
        // the decision.
        try {
          await setUserPref(EVALUATED_KEY, 'true');
        } catch {
          // re-evaluated next launch
        }
        if (fresh) setOpen(true);
      } catch {
        // No answer was obtained, so nothing is recorded and the next launch
        // asks again. This used to mark itself evaluated regardless of the
        // outcome, which meant a launch where the check merely FAILED retired
        // onboarding permanently — exactly what happened on the Android build
        // whose UniFFI bindings were out of step with its native library: every
        // call threw, and the install could never be offered the wizard again.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const onClose = () => setOpen(false);

  // Mounting the modal unconditionally (driven by `visible`) keeps AppDialog's
  // focus-trap mount/unmount lifecycle clean.
  return <FirstLaunchWizardModal visible={open} onClose={onClose} />;
}
