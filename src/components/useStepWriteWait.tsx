import { useCallback, useMemo, useState, type ReactNode } from 'react';

import type { StepWriteWait } from '@aperio/shared';

import { StepWriteWaitDialog } from './StepWriteWait';

/** One frame of the browser, or a tick where there is none. */
function frame(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof requestAnimationFrame === 'function') requestAnimationFrame(() => resolve());
    else setTimeout(resolve, 0);
  });
}

/**
 * The wait for a surface that is a dialog itself: `waitDialog` goes into its
 * JSX beside its own Modal, `wait` around the write. The dialog is closed, and
 * focus is back where it was before it opened, before `wait` settles, so what
 * the caller says or opens next comes after it: closing a Modal and opening or
 * announcing in the same commit lost focus to the page, or the sentence to the
 * focus moving back.
 */
export function useStepWriteWait(): { waitDialog: ReactNode; wait: StepWriteWait } {
  const [open, setOpen] = useState(false);
  const wait = useCallback<StepWriteWait>(async (inSteps, write) => {
    if (!inSteps) return write();
    setOpen(true);
    try {
      return await write();
    } finally {
      setOpen(false);
      // The close commits, then the Modal gives focus back in a microtask.
      await frame();
      await frame();
    }
  }, []);
  const waitDialog = useMemo(() => <StepWriteWaitDialog open={open} />, [open]);
  return { waitDialog, wait };
}
