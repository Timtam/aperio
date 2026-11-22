// One announcement said after a pause, which anything that follows cancels.
//
// The zone picker says its count line once the typing pauses (decision 122b).
// Said late, it talked over the announcement of a choice made within the
// pause (measured 2026-11-22): a choice, a close and every new keystroke
// cancel it first.

/** A pending announcement. */
export interface DelayedSay {
  /** Say `text` after the pause, in place of anything still pending. */
  schedule(text: string): void;
  /** Drop anything pending. */
  cancel(): void;
}

/** Say through `say` after `ms` of quiet. */
export function delayedSay(say: (text: string) => void, ms: number): DelayedSay {
  let pending: ReturnType<typeof setTimeout> | null = null;
  const cancel = () => {
    if (pending !== null) {
      clearTimeout(pending);
      pending = null;
    }
  };
  return {
    schedule(text) {
      cancel();
      pending = setTimeout(() => {
        pending = null;
        say(text);
      }, ms);
    },
    cancel,
  };
}
