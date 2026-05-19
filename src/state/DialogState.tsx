import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
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
  | { kind: 'quickAdd' }
  | { kind: 'colorLabels' };

interface DialogStateValue {
  mode: DialogMode;
  openEventDialog: (
    event?: CalendarEvent | null,
    calendarId?: string,
  ) => void;
  openTaskDialog: (task?: Task | null, listId?: string) => void;
  openQuickAdd: () => void;
  openColorLabels: () => void;
  close: () => void;
}

const DialogStateContext = createContext<DialogStateValue | null>(null);

export function DialogStateProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<DialogMode>({ kind: 'none' });

  // Capture the trigger element *before* the state change. Once we call
  // `setMode`, React immediately renders the shell with `inert`, which
  // makes the browser blur the previously focused control (e.g. a
  // listbox in DayView). By that point `document.activeElement` is
  // already `<body>` and the snapshot would be useless. Capturing here,
  // synchronously inside the event handler, gets us the right element.
  const triggerRef = useRef<HTMLElement | null>(null);

  const captureTrigger = () => {
    const el = document.activeElement;
    triggerRef.current =
      el instanceof HTMLElement && el !== document.body ? el : null;
  };

  const openEventDialog = useCallback(
    (event: CalendarEvent | null = null, calendarId?: string) => {
      captureTrigger();
      setMode({ kind: 'event', event, calendarId });
    },
    [],
  );
  const openTaskDialog = useCallback(
    (task: Task | null = null, listId?: string) => {
      captureTrigger();
      setMode({ kind: 'task', task, listId });
    },
    [],
  );
  const openQuickAdd = useCallback(() => {
    captureTrigger();
    setMode({ kind: 'quickAdd' });
  }, []);

  const openColorLabels = useCallback(() => {
    captureTrigger();
    setMode({ kind: 'colorLabels' });
  }, []);

  const close = useCallback(() => {
    const target = triggerRef.current;
    triggerRef.current = null;
    setMode({ kind: 'none' });
    if (!target) return;
    // Restore focus after the current React commit so the shell has had
    // a chance to drop `inert`. Without the microtask the browser would
    // refuse the focus call while the target's ancestor still claims
    // to be inert.
    queueMicrotask(() => {
      target.focus({ preventScroll: true });
    });
  }, []);

  const value = useMemo<DialogStateValue>(
    () => ({
      mode,
      openEventDialog,
      openTaskDialog,
      openQuickAdd,
      openColorLabels,
      close,
    }),
    [
      mode,
      openEventDialog,
      openTaskDialog,
      openQuickAdd,
      openColorLabels,
      close,
    ],
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
