import { createContext, useContext } from 'react';

/**
 * Toast queue context + consumer hook. Split out of `ToastProvider`
 * so that component file exports only its component (Fast Refresh);
 * the provider implementation + rendering live there.
 */

export type ToastId = string;

export interface ToastUndo {
  /** Button label shown next to the message. Defaults to the
   *  translated `toast.undoLabel` if absent. */
  label?: string;
  /** Called when the user clicks Undo. Awaited so we can show
   *  spinner / error UX in a future iteration; for now we just
   *  dismiss the toast as soon as the click registers. Failures
   *  bubble out — the caller decides whether to log them. */
  action: () => Promise<void> | void;
}

export interface ToastInput {
  /** Free-form text shown to the user. Should already be
   *  translated by the caller — the provider doesn't know the
   *  caller's i18n namespace. */
  message: string;
  /** Optional Undo button. When absent the toast is informational
   *  only (and auto-dismisses on the same timer). */
  undo?: ToastUndo;
  /** Milliseconds to live. Default 10_000. Pass `0` for sticky —
   *  the toast persists until manually dismissed. Useful for
   *  errors that need acknowledgement; not used by the carry-over
   *  flow. */
  durationMs?: number;
}

export interface ToastContextValue {
  showToast: (input: ToastInput) => ToastId;
  dismissToast: (id: ToastId) => void;
}

export const ToastContext = createContext<ToastContextValue | null>(null);

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) {
    throw new Error('useToast must be used inside <ToastProvider>');
  }
  return ctx;
}
