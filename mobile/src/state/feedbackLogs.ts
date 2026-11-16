// The logs the feedback form holds, for the screen that reads them out line
// by line. Not a route param: up to 2 MB has no business in navigation state,
// which React Navigation keeps and Aperio persists. One form exists at a time,
// so one slot is enough; the form sets it whenever its logs change.
//
// The reading screen subscribes rather than reading once: after a relaunch
// both screens come back from the saved navigation state in the same commit,
// before the form has read anything, and the screen has to follow the form
// from "loading" to the text it then sends.

import { useSyncExternalStore } from 'react';

/** The form's logs: not attached, on the way, read, or unreadable. */
export type HeldFeedbackLogs =
  | { state: 'idle' }
  | { state: 'loading' }
  | { state: 'ready'; text: string }
  | { state: 'failed' };

const IDLE: HeldFeedbackLogs = { state: 'idle' };

let held: HeldFeedbackLogs = IDLE;
const listeners = new Set<() => void>();

export function holdFeedbackLogs(next: HeldFeedbackLogs): void {
  held = next;
  listeners.forEach((listener) => listener());
}

export function heldFeedbackLogs(): HeldFeedbackLogs {
  return held;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The form's logs, following every change. */
export function useHeldFeedbackLogs(): HeldFeedbackLogs {
  return useSyncExternalStore(subscribe, heldFeedbackLogs);
}

/** The text as the lines the reading screen lists; a final newline makes no
 *  empty last line. */
export function feedbackLogLines(text: string | null): string[] {
  if (!text) return [];
  const lines = text.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}
