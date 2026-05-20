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
  | { kind: 'accounts' }
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
  openAccounts: () => void;
  openMoveCopy: (target: MoveCopyTarget) => void;
  close: () => void;
}

const DialogStateContext = createContext<DialogStateValue | null>(null);

/**
 * Dialog navigation is a *stack*, not a single slot.
 *
 * When a dialog opens another dialog — e.g. the user picks a reminder
 * from the Ctrl+Shift+R overview and we open the matching event
 * editor — the new dialog goes on top of the old one. Closing it
 * pops back to the previous dialog, mirroring how a deep navigation
 * link returns to its referrer. Closing the bottom-most dialog
 * restores focus to the view that originally triggered the stack
 * (the listbox in WeekView, the search box, etc.).
 *
 * Only the top entry is rendered. The dialogs below are unmounted
 * while their child is on top and re-mount on pop. That trades a
 * tiny bit of state (focused row index inside reminders / search
 * lists) for a much simpler render path; preserving that state
 * across the round-trip can come later if the UX warrants it.
 *
 * One `triggerStack` entry parallels each mode entry: it stores the
 * DOM element that was focused immediately before the push, so the
 * pop knows where to send focus. When the captured element no
 * longer exists in the DOM (because the dialog that hosted it has
 * been unmounted), the focus restore is a no-op and the Modal
 * component's own mount-time focus handling takes over.
 */
export function DialogStateProvider({ children }: { children: ReactNode }) {
  const [stack, setStack] = useState<DialogMode[]>([]);

  // One entry per stack frame: the element that was focused
  // immediately before the push. Lives in a ref so the captures stay
  // synchronous to the event handler.
  const triggerStackRef = useRef<(HTMLElement | null)[]>([]);

  const captureTrigger = () => {
    const el = document.activeElement;
    triggerStackRef.current.push(
      el instanceof HTMLElement && el !== document.body ? el : null,
    );
  };

  const push = useCallback((next: DialogMode) => {
    captureTrigger();
    setStack((s) => [...s, next]);
  }, []);

  const mode: DialogMode =
    stack.length === 0 ? { kind: 'none' } : stack[stack.length - 1];

  const openEventDialog = useCallback(
    (event: CalendarEvent | null = null, options?: OpenEventOptions) => {
      push({
        kind: 'event',
        event,
        calendarId: options?.calendarId,
        defaultDate: options?.defaultDate,
      });
    },
    [push],
  );
  const openTaskDialog = useCallback(
    (task: Task | null = null, options?: OpenTaskOptions) => {
      push({
        kind: 'task',
        task,
        listId: options?.listId,
        defaultDate: options?.defaultDate,
      });
    },
    [push],
  );
  const openQuickAdd = useCallback(() => push({ kind: 'quickAdd' }), [push]);
  const openColorLabels = useCallback(
    () => push({ kind: 'colorLabels' }),
    [push],
  );
  const openSearch = useCallback(() => push({ kind: 'search' }), [push]);
  const openReminders = useCallback(
    () => push({ kind: 'reminders' }),
    [push],
  );
  const openAccounts = useCallback(() => push({ kind: 'accounts' }), [push]);
  const openMoveCopy = useCallback(
    (target: MoveCopyTarget) => push({ kind: 'moveCopy', target }),
    [push],
  );

  const close = useCallback(() => {
    const target = triggerStackRef.current.pop() ?? null;
    setStack((s) => s.slice(0, -1));
    if (!target) return;
    // Restore focus on the next animation frame. queueMicrotask was
    // too eager on Chromium — it ran before the React commit that
    // drops `inert` from the shell, so the focus() call hit an inert
    // ancestor and silently landed on <body>. RAF guarantees the
    // DOM has been mutated; we additionally double-check on the
    // next frame and re-focus if the element is no longer the
    // active one (e.g. because useEvents re-rendered after a
    // mutation between the two frames).
    //
    // When the popped trigger belonged to a parent dialog that is
    // now re-mounting (stack length > 0), the saved element is no
    // longer in the DOM. The `contains` check turns the restore
    // into a no-op and Modal's own mount-time focus handler takes
    // over for the re-mounted dialog.
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
      openAccounts,
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
      openAccounts,
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
