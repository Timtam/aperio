import {
  useCallback,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

import { isSeriesOccurrence } from '@aperio/shared';
import type { CarryableFields, EventGroup } from '@aperio/shared';

import type {
  Account,
  CalendarEvent,
  Contact,
  Section,
  Task,
  TaskCapabilities,
} from '../api/types';
import { focusActiveView } from '../a11y/focusView';
import { DialogStateContext } from './dialogStateContext';
import type { SettingsTabId } from '../components/SettingsDialog';

/** Which slice of a recurring series an edit applies to. */
export type EventEditScope = 'series' | 'occurrence' | 'this_and_future';

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
      /** Pre-fill the start time (HH:mm) when creating — carries the quick-add's
       *  picked time over the "weitere Details" hand-off. */
      defaultTime?: string;
      /** Pre-fill the title when creating (quick-add → "weitere Details"). */
      defaultTitle?: string;
      /** When editing a recurring occurrence, the scope the up-front prompt
       *  resolved to — the editor opens locked to it. Absent ⇒ the editor's
       *  own default ('occurrence'). */
      initialScope?: EventEditScope;
    }
  | {
      // Outlook-style "this occurrence vs the whole series?" prompt shown
      // before the editor opens for a recurring occurrence. Carries the
      // event so the chosen scope can hand off to the event frame.
      kind: 'eventEditScope';
      event: CalendarEvent;
    }
  | {
      kind: 'task';
      task: Task | null;
      listId?: string;
      defaultDate?: string;
      /** Pre-fill the title when creating — used by quick-add → "more
       *  details" so the in-progress title isn't lost on the hand-off. */
      defaultTitle?: string;
    }
  | { kind: 'quickAdd'; defaultDate?: string }
  | { kind: 'quickAddTask'; defaultDate?: string }
  | {
      // The day-activation chooser: "Termin oder Aufgabe?" on a calendar
      // day (double-click / Enter). Routes to the matching quick-add,
      // carrying the activated day forward as `defaultDate`.
      kind: 'createChooser';
      defaultDate?: string;
    }
  | { kind: 'settings'; initialTab?: SettingsTabId }
  | { kind: 'search' }
  | { kind: 'reminders' }
  | { kind: 'moveCopy'; target: MoveCopyTarget }
  | {
      // "These events mean the same appointment" — the grouping dialog for one
      // event (DESIGN-event-groups.md).
      kind: 'eventGroup';
      event: CalendarEvent;
    }
  | {
      // "Carry this change to the other copies?" — asked AFTER an edit to a
      // grouped event was saved (DESIGN-event-groups.md, Stufe 2).
      kind: 'eventGroupCarry';
      group: EventGroup;
      anchor: { calendar_id: string; event_id: string };
      before: CarryableFields;
      after: CarryableFields;
    }
  | { kind: 'planTask'; task: Task }
  | { kind: 'sectionEdit'; listId: string; section: Section | null }
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
  | {
      kind: 'syncAccountsConnect';
      accounts: Account[];
      /** Which story the wizard tells: 'restore' = §19.11 credentials
       *  missing after a sync restore (default); 'repair' = credentials
       *  present but rejected (refresh-error surface). */
      reason?: 'restore' | 'repair';
    }
  | { kind: 'firstLaunchWizard' };

/**
 * Optional context the caller can pass when opening a *create* dialog
 * (i.e. when `event` / `task` is null). Editing an existing row never
 * needs these — its fields come from the row itself.
 */
export interface OpenEventOptions {
  calendarId?: string;
  /** Pre-fill start/end around this date (ISO string). */
  defaultDate?: string;
  /** Pre-fill the start time (HH:mm) — carries the quick-add's picked time over
   *  the "weitere Details" hand-off so the editor keeps it instead of
   *  re-deriving its own default slot. */
  defaultTime?: string;
  /** Pre-fill the title when creating — carries the in-progress title over
   *  from the event quick-add's "weitere Details" hand-off. */
  defaultTitle?: string;
  /** Replace the current top dialog frame instead of stacking on top — used by
   *  the quick-add's "weitere Details" hand-off so the editor inherits the
   *  quick-add's trigger and focus returns to the original opener (the
   *  activated calendar grid) when the editor closes. Create hand-off only. */
  replace?: boolean;
}

export interface OpenTaskOptions {
  listId?: string;
  defaultDate?: string;
  /** Pre-fill the title when creating a new task (quick-add hand-off). */
  defaultTitle?: string;
  /** Replace the current top frame instead of stacking — the quick-add-task
   *  "weitere Details" hand-off, so focus returns to the original opener when
   *  the editor closes. Create hand-off only. */
  replace?: boolean;
}

export interface OpenContactOptions {
  /** Pre-select this contact list when creating a new contact. */
  listId?: string;
}

export interface OpenQuickAddOptions {
  /** YYYY-MM-DD the quick-add should anchor to. For an event this pre-fills
   *  the start day; for a task it pre-fills the scheduled day. */
  defaultDate?: string;
  /** Replace the current top dialog frame instead of stacking on top — used by
   *  the create chooser's hand-off so focus still returns to the original
   *  opener (the activated calendar grid) when the quick-add closes. */
  replace?: boolean;
}

export interface DialogStateValue {
  mode: DialogMode;
  openEventDialog: (
    event?: CalendarEvent | null,
    options?: OpenEventOptions,
  ) => void;
  /** Resolve the recurring-edit scope prompt: swap the top `eventEditScope`
   *  frame for the event editor locked to `scope` (keeps the opener's
   *  focus-return). No-op if the top frame isn't the scope prompt. */
  chooseEventEditScope: (scope: EventEditScope) => void;
  openTaskDialog: (task?: Task | null, options?: OpenTaskOptions) => void;
  /** Quick-add EVENT. `defaultDate` (YYYY-MM-DD) anchors it to a chosen day
   *  (the day-activation chooser / a calendar day); omit to use the view's
   *  focused day. Expands to the full event editor via "weitere Details". */
  openQuickAdd: (options?: OpenQuickAddOptions) => void;
  /** Quick-add TASK. `defaultDate` (YYYY-MM-DD) schedules it on a chosen day;
   *  omit to start it dateless (backlog). Expands to the full task editor. */
  openQuickAddTask: (options?: OpenQuickAddOptions) => void;
  /** Day-activation chooser ("Termin oder Aufgabe?"). `defaultDate`
   *  (YYYY-MM-DD) is the activated calendar day, carried into the chosen
   *  quick-add. */
  openCreateChooser: (defaultDate?: string) => void;
  /**
   * Open the unified Settings dialog. Pass an `initialTab` to land on a
   * specific category — used by the legacy entry points that used to
   * open `AccountsDialog` / `ColorLabelDialog` directly so they keep
   * working without rewriting every call site.
   */
  openSettings: (initialTab?: SettingsTabId) => void;
  /** Persist the Settings dialog's current tab into its stack frame so a
   *  stacked-dialog round trip (e.g. the reconnect wizard) remounts
   *  Settings on the SAME tab instead of resetting to General. */
  recordSettingsTab: (tab: SettingsTabId) => void;
  /** Convenience: open Settings on the Color-labels tab. */
  openColorLabels: () => void;
  /** Convenience: open Settings on the Accounts tab. */
  openAccounts: () => void;
  openSearch: () => void;
  openReminders: () => void;
  openMoveCopy: (target: MoveCopyTarget) => void;
  /** Open the event-group dialog for one event. */
  openEventGroup: (event: CalendarEvent) => void;
  /** Ask whether a just-saved edit should travel to the group's other copies. */
  openEventGroupCarry: (payload: {
    group: EventGroup;
    anchor: { calendar_id: string; event_id: string };
    before: CarryableFields;
    after: CarryableFields;
  }) => void;
  openPlanTask: (task: Task) => void;
  /** Open the create/rename section dialog. Pass `section=null` (or omit)
   *  to create a new section in `listId`; pass an existing Section to
   *  rename / recolor it. Shared by the task-view section-header menu and
   *  the sidebar list menu so both surfaces reach the same accessible name
   *  editor (the only previous entry point was the task editor's Section
   *  field, which wasn't discoverable). */
  openSectionDialog: (listId: string, section?: Section | null) => void;
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
  openSyncAccountsConnect: (
    accounts: Account[],
    reason?: 'restore' | 'repair',
  ) => void;
  /** §19.11 first-launch wizard. Opened by `FirstLaunchWizardChecker` on a
   *  fresh instance: language → sync (restore / create / skip) → first
   *  account. */
  openFirstLaunchWizard: () => void;
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

  // Swap the top stack frame in place WITHOUT capturing a new trigger, so the
  // replacement inherits the original opener's focus-return target. The create
  // chooser uses this to hand off to a quick-add: closing the quick-add still
  // returns focus to the calendar grid the user activated, not to the
  // now-unmounted chooser button (which would strand focus on <body>).
  const replaceTop = useCallback((next: DialogMode) => {
    setStack((s) => (s.length === 0 ? [next] : [...s.slice(0, -1), next]));
  }, []);

  const mode: DialogMode = useMemo(
    () => (stack.length === 0 ? { kind: 'none' } : stack[stack.length - 1]),
    [stack],
  );

  const openEventDialog = useCallback(
    (event: CalendarEvent | null = null, options?: OpenEventOptions) => {
      // Editing one occurrence of a recurring series: ask up front whether the
      // edit targets this occurrence or the whole series (Outlook-style), then
      // hand off to the editor locked to that scope. Everything else — creating,
      // or editing a non-recurring / master row — opens the editor directly.
      if (event && isSeriesOccurrence(event)) {
        push({ kind: 'eventEditScope', event });
        return;
      }
      const next: DialogMode = {
        kind: 'event',
        event,
        calendarId: options?.calendarId,
        defaultDate: options?.defaultDate,
        defaultTime: options?.defaultTime,
        defaultTitle: options?.defaultTitle,
      };
      // The quick-add "weitere Details" hand-off swaps its own frame for the
      // editor (no new trigger capture, no close()) so the editor inherits the
      // grid trigger and focus returns there on close — matching the
      // createChooser → quick-add and eventEditScope → editor hand-offs.
      if (options?.replace) replaceTop(next);
      else push(next);
    },
    [push, replaceTop],
  );
  const chooseEventEditScope = useCallback((scope: EventEditScope) => {
    // Swap the scope-prompt frame in place (no new trigger capture) so the
    // editor inherits the opener's focus-return target, mirroring the
    // createChooser → quick-add hand-off.
    setStack((s) => {
      const top = s[s.length - 1];
      if (!top || top.kind !== 'eventEditScope') return s;
      return [
        ...s.slice(0, -1),
        { kind: 'event', event: top.event, initialScope: scope },
      ];
    });
  }, []);
  const openTaskDialog = useCallback(
    (task: Task | null = null, options?: OpenTaskOptions) => {
      const next: DialogMode = {
        kind: 'task',
        task,
        listId: options?.listId,
        defaultDate: options?.defaultDate,
        defaultTitle: options?.defaultTitle,
      };
      // Quick-add-task "weitere Details" hand-off — replace the frame in place
      // (see openEventDialog) so focus returns to the original opener on close.
      if (options?.replace) replaceTop(next);
      else push(next);
    },
    [push, replaceTop],
  );
  const openQuickAdd = useCallback(
    (options?: OpenQuickAddOptions) => {
      const next: DialogMode = {
        kind: 'quickAdd',
        defaultDate: options?.defaultDate,
      };
      if (options?.replace) replaceTop(next);
      else push(next);
    },
    [push, replaceTop],
  );
  const openQuickAddTask = useCallback(
    (options?: OpenQuickAddOptions) => {
      const next: DialogMode = {
        kind: 'quickAddTask',
        defaultDate: options?.defaultDate,
      };
      if (options?.replace) replaceTop(next);
      else push(next);
    },
    [push, replaceTop],
  );
  const openCreateChooser = useCallback(
    (defaultDate?: string) => push({ kind: 'createChooser', defaultDate }),
    [push],
  );
  const openSettings = useCallback(
    (initialTab?: SettingsTabId) => push({ kind: 'settings', initialTab }),
    [push],
  );
  // Record the Settings dialog's CURRENT tab in its stack frame. Only the
  // top of the stack renders, so pushing another dialog on top (reconnect
  // wizard, OAuth guide) unmounts Settings; on return it remounts from the
  // frame and would otherwise reset to the General tab mid-task.
  const recordSettingsTab = useCallback(
    (tab: SettingsTabId) =>
      setStack((s) =>
        s.some((f) => f.kind === 'settings' && f.initialTab !== tab)
          ? s.map((f) =>
              f.kind === 'settings' ? { ...f, initialTab: tab } : f,
            )
          : s,
      ),
    [],
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
  const openEventGroup = useCallback(
    (event: CalendarEvent) => push({ kind: 'eventGroup', event }),
    [push],
  );
  const openEventGroupCarry = useCallback(
    (payload: {
      group: EventGroup;
      anchor: { calendar_id: string; event_id: string };
      before: CarryableFields;
      after: CarryableFields;
    }) => push({ kind: 'eventGroupCarry', ...payload }),
    [push],
  );
  const openPlanTask = useCallback(
    (task: Task) => push({ kind: 'planTask', task }),
    [push],
  );
  const openSectionDialog = useCallback(
    (listId: string, section: Section | null = null) =>
      push({ kind: 'sectionEdit', listId, section }),
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
    (accounts: Account[], reason?: 'restore' | 'repair') =>
      push({ kind: 'syncAccountsConnect', accounts, reason }),
    [push],
  );
  const openFirstLaunchWizard = useCallback(
    () => push({ kind: 'firstLaunchWizard' }),
    [push],
  );

  const close = useCallback(() => {
    const target = triggerStackRef.current.pop() ?? null;
    // The trigger stack is parallel to the dialog stack, so once we've popped
    // this frame's trigger an empty stack means we're returning to the view (no
    // parent dialog underneath). Only then may we fall back to the view; when a
    // parent dialog remains, its own Modal focus handling takes over.
    const returningToView = triggerStackRef.current.length === 0;
    setStack((s) => s.slice(0, -1));
    // Closing a dialog is the canonical "data may have changed" hint
    // — the user just confirmed an edit / create / delete. Bump
    // unconditionally so useEvents / useTasks refetch on the next
    // tick. Doing it here keeps every existing call site working
    // without remembering to call invalidateData() itself.
    setDataVersion((v) => v + 1);
    // Restore focus on the next animation frame. queueMicrotask was
    // too eager on Chromium — it ran before the React commit that
    // drops `inert` from the shell, so the focus() call hit an inert
    // ancestor and silently landed on <body>. RAF guarantees the
    // DOM has been mutated; we additionally double-check on the
    // next frame and re-focus if the element is no longer the
    // active one (e.g. because useEvents re-rendered after a
    // mutation between the two frames).
    const restore = () => {
      if (target && document.body.contains(target)) {
        target.focus({ preventScroll: true });
        return;
      }
      // The trigger is null (a GLOBAL shortcut like Alt+N fired while focus was
      // already on <body>) or was unmounted by the post-close re-render. If we're
      // returning to a view, land focus on the view's focusable container instead
      // of leaving it stranded on <body> — which sits OUTSIDE #app-root's
      // role="application" and drops the screen reader out of application mode.
      // When a parent dialog is re-mounting (returningToView false), the saved
      // element being gone is expected — leave it to that Modal's focus handler.
      if (returningToView) focusActiveView();
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
      chooseEventEditScope,
      openTaskDialog,
      openQuickAdd,
      openQuickAddTask,
      openCreateChooser,
      openSettings,
      recordSettingsTab,
      openColorLabels,
      openAccounts,
      openSearch,
      openReminders,
      openMoveCopy,
      openEventGroup,
      openEventGroupCarry,
      openPlanTask,
      openSectionDialog,
      openTaskMembers,
      openDayStartReview,
      openContactDialog,
      openSyncConflicts,
      openSyncSchemaTooOld,
      openSyncStaleResume,
      openSyncAccountsConnect,
      openFirstLaunchWizard,
      close,
      dataVersion,
      invalidateData,
    }),
    [
      mode,
      openEventDialog,
      chooseEventEditScope,
      openTaskDialog,
      openQuickAdd,
      openQuickAddTask,
      openCreateChooser,
      openSettings,
      recordSettingsTab,
      openColorLabels,
      openAccounts,
      openSearch,
      openReminders,
      openMoveCopy,
      openEventGroup,
      openEventGroupCarry,
      openPlanTask,
      openSectionDialog,
      openTaskMembers,
      openDayStartReview,
      openContactDialog,
      openSyncConflicts,
      openSyncSchemaTooOld,
      openSyncStaleResume,
      openSyncAccountsConnect,
      openFirstLaunchWizard,
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

