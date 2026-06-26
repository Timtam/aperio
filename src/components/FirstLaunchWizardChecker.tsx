import { useEffect, useRef } from 'react';

import { isFreshInstance } from '../api/client';
import { useDialogState } from '../state/dialogStateContext';

/** Device-local marker that the first-launch check has already run, so an
 *  empty instance that dismissed (or was never offered) the wizard isn't
 *  re-evaluated on every launch. */
const EVALUATED_KEY = 'aperio.firstLaunchWizard.evaluated';

function readEvaluated(): boolean {
  try {
    return localStorage.getItem(EVALUATED_KEY) === 'true';
  } catch {
    return false;
  }
}

function writeEvaluated(): void {
  try {
    localStorage.setItem(EVALUATED_KEY, 'true');
  } catch {
    // storage unavailable — we just re-evaluate next launch (cheap)
  }
}

/**
 * Startup gate for the first-launch wizard (DESIGN.md §19.11). On the first
 * launch of a genuinely FRESH instance — no external account, no sync target,
 * an empty local store (the backend `is_fresh_instance` check) — it opens the
 * wizard exactly once, paired with a device-local "evaluated" marker so an
 * established (or already-dismissed) install is never prompted. Renders nothing.
 */
export function FirstLaunchWizardChecker() {
  const { mode, openFirstLaunchWizard } = useDialogState();
  const evaluatedRef = useRef(readEvaluated());

  useEffect(() => {
    if (evaluatedRef.current) return;
    // Don't stack on top of another startup dialog; re-run once it closes.
    if (mode.kind !== 'none') return;
    let cancelled = false;
    void isFreshInstance()
      .then((fresh) => {
        if (cancelled) return;
        // Mark evaluated BEFORE opening so the mode change can't re-trigger us.
        evaluatedRef.current = true;
        writeEvaluated();
        if (fresh) openFirstLaunchWizard();
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('is_fresh_instance failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, [mode.kind, openFirstLaunchWizard]);

  return null;
}
