// Imperative bridge for the "please wait" dialog while a series is created in
// steps (decisions 227, 265): the calendar takes a new series without its
// deleted occurrences, and the adapter removes them again one by one, which can
// take up to half a minute. The editor, the carry and Move/Copy call `stepWriteWait`
// around such a write, and each renders its own <StepWriteWaitHost /> inside
// its screen: on iOS a dialog presents from the screen it is rendered in, and
// one rendered at the app root could not present over a screen shown as a
// sheet. Kept apart from the host COMPONENT so that file only exports a
// component (Fast Refresh). Mirrors the desktop's useStepWriteWait.

import type { StepWriteWait } from '@aperio/shared';

/** What the mounted host does: show the dialog, and hide it again — settling
 *  only once it is gone, so what the caller presents or says next does not
 *  race its dismissal. */
export interface StepWriteWaitHost {
  open: () => void;
  close: () => Promise<void>;
}

// Every mounted host, in the order they mounted. A screen opens above the one
// that opened it and mounts after it, so the last is the screen on top: the
// one whose write this is.
const hosts: StepWriteWaitHost[] = [];
/** The host the dialog is open in, until it is closed. */
let openIn: StepWriteWaitHost | null = null;
let held = 0;

/** A host registers itself here; returns an unsubscribe. */
export function registerStepWriteWaitHost(next: StepWriteWaitHost): () => void {
  hosts.push(next);
  return () => {
    const at = hosts.lastIndexOf(next);
    if (at !== -1) hosts.splice(at, 1);
    // Gone with its screen, and its dialog with it: nothing is left to close.
    if (openIn === next) openIn = null;
  };
}

async function closeIfOpen(): Promise<void> {
  const host = openIn;
  if (host == null) return;
  openIn = null;
  await host.close();
}

/**
 * The wait every phone surface uses: `write` runs either way; where `inSteps`
 * holds, the dialog is up while it runs and gone before this settles. Without
 * a mounted host the write just runs.
 */
export const stepWriteWait: StepWriteWait = async (inSteps, write) => {
  const top = hosts[hosts.length - 1];
  if (!inSteps || (openIn == null && top == null)) return write();
  if (openIn == null && top != null) {
    openIn = top;
    top.open();
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
