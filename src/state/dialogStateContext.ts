import { createContext, useContext } from 'react';

import type { DialogStateValue } from './DialogState';

/**
 * Dialog-stack context + consumer hook. Split out of
 * `DialogStateProvider` so that component file exports only its
 * component (Fast Refresh). The value type + the provider
 * implementation live alongside the component.
 */
export const DialogStateContext = createContext<DialogStateValue | null>(null);

export function useDialogState(): DialogStateValue {
  const ctx = useContext(DialogStateContext);
  if (!ctx) {
    throw new Error('useDialogState must be used inside <DialogStateProvider>');
  }
  return ctx;
}
