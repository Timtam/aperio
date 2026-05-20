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
export type MoveCopyTarget =
  | { kind: 'event'; event: CalendarEvent }
  | { kind: 'task'; task: Task };

export type DialogMode =
  | { kind: 'none' }
  | {
      kind: 'event';
      event: CalendarEvent | null;
      calendarId?: string;
      /** Pre-fill start/end around this date when creating a new event. */
      defaultDate?: string;
    }
  | {
      kind: 'task';
      task: Task | null;
      listId?: string;
      defaultDate?: string;
    }
  | { kind: 'quickAdd' }
  | { kind: 'colorLabels' }
  | { kind: 'search' }
  | { kind: 'reminders' }
  | { kind: 'moveCopy'; target: MoveCopyTarget };

/**
 * Optional context the caller can pass when opening a *create* dialog
 * (i.e. when `event` / `task` is null). Editing an existing row never
 * needs these — its fields come from the row itself.
 */
export interface OpenEventOptions {
  calendarId?: string;
  /** Pre-fill start/end around this date (ISO string). */
  defaultDate?: string;
}

export interface OpenTaskOptions {
  listId?: string;
  defaultDate?: string;
}

interface DialogStateValue {
  mode: DialogMode;
  openEventDialog: (
    event?: CalendarEvent | null,
    options?: OpenEventOptions,
  ) => void;
  openTaskDialog: (task?: Task | null, options?: OpenTaskOptions) => void;
  openQuickAdd: () => void;
  openColorLabels: () => void;
  openSearch: () => void;
  openReminders: () => void;
  openMoveCopy: (target: MoveCopyTarget) => void;
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
    (event: CalendarEvent | null = null, options?: OpenEventOptions) => {
      captureTrigger();
      setMode({
        kind: 'event',
        event,
        calendarId: options?.calendarId,
        defaultDate: options?.defaultDate,
      });
    },
    [],
  );
  const openTaskDialog = useCallback(
    (task: Task | null = null, options?: OpenTaskOptions) => {
      captureTrigger();
      setMode({
        kind: 'task',
        task,
        listId: options?.listId,
        defaultDate: options?.defaultDate,
      });
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

  const openSearch = useCallback(() => {
    captureTrigger();
    setMode({ kind: 'search' });
  }, []);

  const openReminders = useCallback(() => {
    captureTrigger();
    setMode({ kind: 'reminders' });
  }, []);

  const openMoveCopy = useCallback((target: MoveCopyTarget) => {
    captureTrigger();
    setMode({ kind: 'moveCopy', target });
  }, []);

  const close = useCallback(() => {
    const target = triggerRef.current;
    triggerRef.current = null;
    setMode({ kind: 'none' });
    if (!target) return;
    // Restore focus on the next animation frame. queueMicrotask was too
    // eager on Chromium — it ran before the React commit that drops
    // `inert` from the shell, so the focus() call hit an inert
    // ancestor and silently landed on <body>. RAF guarantees the DOM
    // has been mutated; we additionally double-check on the *next*
    // frame and re-focus if the element is no longer the active one,
    // because re-fetches triggered by the dialog close (useEvents,
    // useTasks) can re-render between the two frames.
    const restore = () => {
      if (!document.body.contains(target)) return;
      target.focus({ preventScroll: true });
    };
    requestAnimationFrame(() => {
      restore();
      requestAnimationFrame(() => {
        if (document.activeElement !== target) restore();
      });
    });
  }, []);

  const value = useMemo<DialogStateValue>(
    () => ({
      mode,
      openEventDialog,
      openTaskDialog,
      openQuickAdd,
      openColorLabels,
      openSearch,
      openReminders,
      openMoveCopy,
      close,
    }),
    [
      mode,
      openEventDialog,
      openTaskDialog,
      openQuickAdd,
      openColorLabels,
      openSearch,
      openReminders,
      openMoveCopy,
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
