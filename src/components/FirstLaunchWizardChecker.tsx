import { useEffect, useRef } from 'react';

import { getUserPref, isFreshInstance, setUserPref } from '../api/client';
import { useDialogState } from '../state/dialogStateContext';

/** Marker that the first-launch check has already run, so an empty instance
 *  that dismissed (or was never offered) the wizard isn't re-evaluated on every
 *  launch.
 *
 *  It lives in `user_prefs`, INSIDE the data directory, and that placement is
 *  the whole point. It used to sit in `localStorage`, which on the desktop is
 *  the WebView's own profile directory — somewhere else entirely. Deleting the
 *  data directory to start over therefore did not clear it, so the wizard was
 *  offered once in the lifetime of the machine and never again, however empty
 *  the instance was. Now a wiped data directory really is a first launch.
 *
 *  Still device-local: `onboarding.` is absent from the sync whitelist
 *  (`crates/sync-engine/src/whitelist.rs`), so the row stays on this device the
 *  way the old one did. Do not add it there — each device decides for itself
 *  whether it has been through onboarding. */
const EVALUATED_KEY = 'onboarding.firstLaunchWizardEvaluated';

/**
 * Startup gate for the first-launch wizard (DESIGN.md §19.11). On the first
 * launch of a genuinely FRESH instance — no external account, no sync target,
 * an empty local store (the backend `is_fresh_instance` check) — it opens the
 * wizard exactly once, paired with the marker above so an established (or
 * already-dismissed) install is never prompted. Renders nothing.
 */
export function FirstLaunchWizardChecker() {
  const { mode, openFirstLaunchWizard } = useDialogState();
  const evaluatedRef = useRef(false);

  useEffect(() => {
    if (evaluatedRef.current) return;
    // Don't stack on top of another startup dialog; re-run once it closes.
    if (mode.kind !== 'none') return;
    let cancelled = false;

    void (async () => {
      try {
        if ((await getUserPref(EVALUATED_KEY)) === 'true') {
          evaluatedRef.current = true;
          return;
        }
        const fresh = await isFreshInstance();
        if (cancelled) return;
        // Mark evaluated BEFORE opening so the mode change can't re-trigger us.
        evaluatedRef.current = true;
        // A failed write costs one redundant evaluation next launch, which is
        // cheap. It must not cost the user the wizard, so it cannot sit between
        // the answer and the decision.
        try {
          await setUserPref(EVALUATED_KEY, 'true');
        } catch {
          // re-evaluated next launch
        }
        if (fresh) openFirstLaunchWizard();
      } catch (err) {
        // No answer was obtained — leave the marker unwritten so the next
        // launch asks again. Writing it here would retire onboarding for this
        // install on the strength of a failed call.
        // eslint-disable-next-line no-console
        console.warn('first-launch check failed', err);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [mode.kind, openFirstLaunchWizard]);

  return null;
}
