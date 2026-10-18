import { useCallback, useMemo, useRef, useState, type ReactNode } from 'react';

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
 *
 * `hold` keeps it open across several writes — the copies a carry writes one
 * after the other —, so it opens at the first that needs it and says so once;
 * the release it returns closes it, again before it settles.
 */
export function useStepWriteWait(): {
  waitDialog: ReactNode;
  wait: StepWriteWait;
  hold: () => () => Promise<void>;
} {
  const [open, setOpen] = useState(false);
  const opened = useRef(false);
  const held = useRef(0);
  const close = useCallback(async () => {
    opened.current = false;
    setOpen(false);
    // The close commits, then the Modal gives focus back in a microtask.
    await frame();
    await frame();
  }, []);
  const wait = useCallback<StepWriteWait>(
    async (inSteps, write) => {
      if (!inSteps) return write();
      if (!opened.current) {
        opened.current = true;
        setOpen(true);
      }
      if (held.current > 0) return write();
      try {
        return await write();
      } finally {
        await close();
      }
    },
    [close],
  );
  const hold = useCallback(() => {
    held.current += 1;
    let released = false;
    return async () => {
      if (released) return;
      released = true;
      held.current -= 1;
      if (held.current === 0 && opened.current) await close();
    };
  }, [close]);
  const waitDialog = useMemo(() => <StepWriteWaitDialog open={open} />, [open]);
  return { waitDialog, wait, hold };
}
