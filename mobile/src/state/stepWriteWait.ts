// Imperative bridge for the "please wait" dialog while a series is created in
// steps (decisions 227, 265): Exchange takes a new series without its deleted
// occurrences, and the adapter deletes them again one by one, which can take up
// to half a minute. The editor, the carry and Move/Copy call `stepWriteWait`
// around such a write; the single <StepWriteWaitHost /> mounted at the app root
// renders the dialog. Kept apart from the host COMPONENT so that file only
// exports a component (Fast Refresh). Mirrors the desktop's useStepWriteWait.

import type { StepWriteWait } from '@aperio/shared';

/** What the mounted host does: show the dialog, and hide it again — settling
 *  only once it is gone, so what the caller presents or says next does not
 *  race its dismissal. */
export interface StepWriteWaitHost {
  open: () => void;
  close: () => Promise<void>;
}

let host: StepWriteWaitHost | null = null;
let opened = false;
let held = 0;

/** The host registers itself here; returns an unsubscribe. */
export function registerStepWriteWaitHost(next: StepWriteWaitHost): () => void {
  host = next;
  return () => {
    if (host === next) host = null;
  };
}

async function closeIfOpen(): Promise<void> {
  if (!opened) return;
  opened = false;
  await host?.close();
}

/**
 * The wait every phone surface uses: `write` runs either way; where `inSteps`
 * holds, the dialog is up while it runs and gone before this settles. Without
 * a mounted host the write just runs.
 */
export const stepWriteWait: StepWriteWait = async (inSteps, write) => {
  if (!inSteps || host == null) return write();
  if (!opened) {
    opened = true;
    host.open();
  }
  if (held > 0) return write();
  try {
    return await write();
  } finally {
    await closeIfOpen();
  }
};

/**
 * Keep the dialog up across several writes — the copies a carry writes one
 * after the other —, so it opens at the first that needs it and says itself
 * once; the release closes it, settling once it is gone.
 */
export function holdStepWriteWait(): () => Promise<void> {
  held += 1;
  let released = false;
  return async () => {
    if (released) return;
    released = true;
    held -= 1;
    if (held === 0) await closeIfOpen();
  };
}
