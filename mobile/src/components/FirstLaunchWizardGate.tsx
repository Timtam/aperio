import AsyncStorage from '@react-native-async-storage/async-storage';
import { useEffect, useRef, useState } from 'react';

import { isFreshInstance } from '../api/onboarding';
import { FirstLaunchWizardModal } from './FirstLaunchWizardModal';

/** Device-local marker that the first-launch check has already run, so an empty
 *  instance that dismissed (or was never offered) the wizard isn't re-evaluated
 *  on every launch. Not synced — each device runs its own check. */
const EVALUATED_KEY = 'aperio.firstLaunchWizard.evaluated';

/**
 * Startup gate for the first-launch wizard (DESIGN.md §19.11), the mobile twin
 * of the desktop `FirstLaunchWizardChecker`. On the first launch of a genuinely
 * FRESH instance — no external account, no sync target, an empty local store
 * (the `is_fresh_instance` composition) — it opens the wizard once, paired with
 * a device-local "evaluated" marker so an established (or already-dismissed)
 * install is never prompted.
 */
export function FirstLaunchWizardGate() {
  const [open, setOpen] = useState(false);
  const evaluatedRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        if ((await AsyncStorage.getItem(EVALUATED_KEY)) === 'true') return;
      } catch {
        // Storage read failed — fall through and evaluate (cheap); we just
        // won't be able to persist the marker below.
      }
      let fresh = false;
      try {
        fresh = await isFreshInstance();
      } catch {
        // On any error, treat as not-fresh (don't pop the wizard on a
        // half-initialised store).
        fresh = false;
      }
      if (cancelled) return;
      evaluatedRef.current = true;
      // Mark evaluated regardless of the outcome so we never re-query.
      await AsyncStorage.setItem(EVALUATED_KEY, 'true').catch(() => undefined);
      if (fresh) setOpen(true);
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
