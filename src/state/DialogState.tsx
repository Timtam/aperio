import {
  useCallback,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

import type {
  Account,
  CalendarEvent,
  Contact,
  Task,
  TaskCapabilities,
} from '../api/types';
import { DialogStateContext } from './dialogStateContext';
import type { SettingsTabId } from '../components/SettingsDialog';

/**
 * Which dialog (if any) is currently open.
 *
 * `event`/`task` carry the row to edit; `null` means "create new". The
 * provider exposes one open/close call per dialog type so callers don't
 * have to know about the internals — the render path picks the right
 * component to show.
 */
export type MoveCopyTarget =
  | {
      kind: 'event';
      event: CalendarEvent;
      /** Optional initial mode for the dialog's Move / Copy radio.
       *  Defaults to `move` to match the historical Shift+M shortcut.
       *  Context-menu callers pass `copy` for "Kopieren nach …". */
      defaultMode?: 'move' | 'copy';
    }
  | { kind: 'task'; task: Task; defaultMode?: 'move' | 'copy' };

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
  | { kind: 'quickAddTask' }
  | { kind: 'settings'; initialTab?: SettingsTabId }
  | { kind: 'search' }
  | { kind: 'reminders' }
  | { kind: 'moveCopy'; target: MoveCopyTarget }
  | { kind: 'planTask'; task: Task }
  | {
      kind: 'taskMembers';
      listId: string;
      listName: string;
      /** Drives the dialog's add control (search vs email) + role/pending
       *  affordances. Absent ⇒ the dialog falls back to search + roles. */
      capabilities?: TaskCapabilities;
    }
  | { kind: 'dayStartReview' }
  | {
      kind: 'contact';
      contact: Contact | null;
      /** Pre-select this contact list when creating a new contact.
       *  Ignored when editing (the contact's own `list_id` wins). */
      listId?: string;
    }
  | { kind: 'syncConflicts' }
  | { kind: 'syncSchemaTooOld'; required: string; running: string }
  | { kind: 'syncStaleResume'; snapshotAt: string }
  | { kind: 'syncAccountsConnect'; accounts: Account[] };

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

export interface OpenContactOptions {
  /** Pre-select this contact list when creating a new contact. */
  listId?: string;
}

export interface DialogStateValue {
  mode: DialogMode;
  openEventDialog: (
    event?: CalendarEvent | null,
    options?: OpenEventOptions,
  ) => void;
  openTaskDialog: (task?: Task | null, options?: OpenTaskOptions) => void;
  openQuickAdd: () => void;
  openQuickAddTask: () => void;
  /**
   * Open the unified Settings dialog. Pass an `initialTab` to land on a
   * specific category — used by the legacy entry points that used to
   * open `AccountsDialog` / `ColorLabelDialog` directly so they keep
   * working without rewriting every call site.
   */
  openSettings: (initialTab?: SettingsTabId) => void;
  /** Convenience: open Settings on the Color-labels tab. */
  openColorLabels: () => void;
  /** Convenience: open Settings on the Accounts tab. */
  openAccounts: () => void;
  openSearch: () => void;
  openReminders: () => void;
  openMoveCopy: (target: MoveCopyTarget) => void;
  openPlanTask: (task: Task) => void;
  /** Open the task-list membership/sharing dialog (DESIGN §9.7). Pass the
   *  list's `task_capabilities` so the dialog can switch between
   *  search-add (Vikunja) and email-invite (Todoist). */
  openTaskMembers: (
    listId: string,
    listName: string,
    capabilities?: TaskCapabilities,
  ) => void;
  /**
   * Open the unified day-start review (DESIGN.md § 9.5). One dialog
   * with two sections — deadline overruns + schedule slips — replaces
   * the old MissedTasksDialog + CarryOverDialog pair.
   */
  openDayStartReview: () => void;
  /** Open the contact create/edit dialog. Pass `null` (or omit) for
   *  a fresh contact, an existing `Contact` to edit it. */
  openContactDialog: (
    contact?: Contact | null,
    options?: OpenContactOptions,
  ) => void;
  /** Open the §19.3 conflict-resolution dialog. The dialog reads its
   *  list from `list_sync_conflicts` on mount + on every
   *  `sync-conflicts-changed` event. */
  openSyncConflicts: () => void;
  /** Pop the §19.13 "update required" dialog. Non-dismissible —
   *  the user picks "Update" or "Offline fortfahren". Mounted
   *  automatically by `useSync` when the backend latches
   *  `schema_too_old`. */
  openSyncSchemaTooOld: (required: string, running: string) => void;
  /** §19.10 stale-device resume dialog. Mounted by `useSync`
   *  when the backend latches `status.stale_device_since`. The
   *  user clicks Fortfahren → resume command → latch clears. */
  openSyncStaleResume: (snapshotAt: string) => void;
  /** §19.11 step 8 — "Konten verbinden" wizard. Opened by the
   *  SyncPanel right after a successful `accept_remote_dataset`
   *  when the snapshot brought in account rows that don't yet
   *  have their secrets on this device. The dialog walks the
   *  user through re-attaching credentials per account. */
  openSyncAccountsConnect: (accounts: Account[]) => void;
  close: () => void;
  /**
   * Counter that bumps whenever data on the wire might have changed.
   * Consumers like `useEvents` / `useTasks` watch it as a dependency
   * so they refetch after any mutation — including ones that happen
   * outside a globally-tracked dialog (e.g. the DeleteEventScope or
   * Confirm dialogs that live inside a view's local state).
   *
   * `close()` automatically bumps this so the common case (mutate
   * inside a dialog, close the dialog) is covered without every
   * call site remembering. View-level mutation handlers that
   * never open a global dialog (per-row Delete shortcut, in-place
   * status toggle) need to call `invalidateData()` themselves
   * after the await resolves.
   */
  dataVersion: number;
  invalidateData: () => void;
}

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
  const [dataVersion, setDataVersion] = useState(0);
  const invalidateData = useCallback(
    () => setDataVersion((v) => v + 1),
    [],
  );

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

  const mode: DialogMode = useMemo(
    () => (stack.length === 0 ? { kind: 'none' } : stack[stack.length - 1]),
    [stack],
  );

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
  const openQuickAddTask = useCallback(
    () => push({ kind: 'quickAddTask' }),
    [push],
  );
  const openSettings = useCallback(
    (initialTab?: SettingsTabId) => push({ kind: 'settings', initialTab }),
    [push],
  );
  const openColorLabels = useCallback(
    () => openSettings('colorLabels'),
    [openSettings],
  );
  const openAccounts = useCallback(
    () => openSettings('accounts'),
    [openSettings],
  );
  const openSearch = useCallback(() => push({ kind: 'search' }), [push]);
  const openReminders = useCallback(
    () => push({ kind: 'reminders' }),
    [push],
  );
  const openMoveCopy = useCallback(
    (target: MoveCopyTarget) => push({ kind: 'moveCopy', target }),
    [push],
  );
  const openPlanTask = useCallback(
    (task: Task) => push({ kind: 'planTask', task }),
    [push],
  );
  const openTaskMembers = useCallback(
    (listId: string, listName: string, capabilities?: TaskCapabilities) =>
      push({ kind: 'taskMembers', listId, listName, capabilities }),
    [push],
  );
  const openDayStartReview = useCallback(
    () => push({ kind: 'dayStartReview' }),
    [push],
  );
  const openContactDialog = useCallback(
    (contact: Contact | null = null, options?: OpenContactOptions) => {
      push({
        kind: 'contact',
        contact,
        listId: options?.listId,
      });
    },
    [push],
  );
  const openSyncConflicts = useCallback(
    () => push({ kind: 'syncConflicts' }),
    [push],
  );
  const openSyncSchemaTooOld = useCallback(
    (required: string, running: string) =>
      push({ kind: 'syncSchemaTooOld', required, running }),
    [push],
  );
  const openSyncStaleResume = useCallback(
    (snapshotAt: string) => push({ kind: 'syncStaleResume', snapshotAt }),
    [push],
  );
  const openSyncAccountsConnect = useCallback(
    (accounts: Account[]) => push({ kind: 'syncAccountsConnect', accounts }),
    [push],
  );

  const close = useCallback(() => {
    const target = triggerStackRef.current.pop() ?? null;
    setStack((s) => s.slice(0, -1));
    // Closing a dialog is the canonical "data may have changed" hint
    // — the user just confirmed an edit / create / delete. Bump
    // unconditionally so useEvents / useTasks refetch on the next
    // tick. Doing it here keeps every existing call site working
    // without remembering to call invalidateData() itself.
    setDataVersion((v) => v + 1);
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
      openQuickAddTask,
      openSettings,
      openColorLabels,
      openAccounts,
      openSearch,
      openReminders,
      openMoveCopy,
      openPlanTask,
      openTaskMembers,
      openDayStartReview,
      openContactDialog,
      openSyncConflicts,
      openSyncSchemaTooOld,
      openSyncStaleResume,
      openSyncAccountsConnect,
      close,
      dataVersion,
      invalidateData,
    }),
    [
      mode,
      openEventDialog,
      openTaskDialog,
      openQuickAdd,
      openQuickAddTask,
      openSettings,
      openColorLabels,
      openAccounts,
      openSearch,
      openReminders,
      openMoveCopy,
      openPlanTask,
      openTaskMembers,
      openDayStartReview,
      openContactDialog,
      openSyncConflicts,
      openSyncSchemaTooOld,
      openSyncStaleResume,
      openSyncAccountsConnect,
      close,
      dataVersion,
      invalidateData,
    ],
  );

  return (
    <DialogStateContext.Provider value={value}>
      {children}
    </DialogStateContext.Provider>
  );
}

