import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import type { CalendarEvent, Task } from '../api/types';

/**
 * Which dialog (if any) is currently open.
 *
 * `event`/`task` carry the row to edit; `null` means "create new". The
 * provider exposes one open/close call per dialog type so callers don't
 * have to know about the internals — the render path picks the right
 * component to show.
 */
export type DialogMode =
  | { kind: 'none' }
  | { kind: 'event'; event: CalendarEvent | null; calendarId?: string }
  | { kind: 'task'; task: Task | null; listId?: string }
  | { kind: 'quickAdd' };

interface DialogStateValue {
  mode: DialogMode;
  openEventDialog: (
    event?: CalendarEvent | null,
    calendarId?: string,
  ) => void;
  openTaskDialog: (task?: Task | null, listId?: string) => void;
  openQuickAdd: () => void;
  close: () => void;
}

const DialogStateContext = createContext<DialogStateValue | null>(null);

export function DialogStateProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<DialogMode>({ kind: 'none' });

  const openEventDialog = useCallback(
    (event: CalendarEvent | null = null, calendarId?: string) =>
      setMode({ kind: 'event', event, calendarId }),
    [],
  );
  const openTaskDialog = useCallback(
    (task: Task | null = null, listId?: string) =>
      setMode({ kind: 'task', task, listId }),
    [],
  );
  const openQuickAdd = useCallback(
    () => setMode({ kind: 'quickAdd' }),
    [],
  );
  const close = useCallback(() => setMode({ kind: 'none' }), []);

  const value = useMemo<DialogStateValue>(
    () => ({ mode, openEventDialog, openTaskDialog, openQuickAdd, close }),
    [mode, openEventDialog, openTaskDialog, openQuickAdd, close],
  );

  return (
    <DialogStateContext.Provider value={value}>
      {children}
    </DialogStateContext.Provider>
  );
}

export function useDialogState(): DialogStateValue {
  const ctx = useContext(DialogStateContext);
  if (!ctx) {
    throw new Error('useDialogState must be used inside <DialogStateProvider>');
  }
  return ctx;
}
