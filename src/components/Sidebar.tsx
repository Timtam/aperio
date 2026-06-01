import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { ConfirmDialog } from './ConfirmDialog';
import { PromptDialog } from './PromptDialog';
import {
  clearContainerNameOverride,
  createCalendar,
  createContactList,
  createTaskList,
  deleteCalendar,
  deleteContactList,
  deleteTaskList,
  isCommandError,
  renameContactList,
  renameContainer,
  reparentTaskList,
  showContextMenu,
  type ContainerKind,
  type ContextMenuItemRequest,
} from '../api/client';
import { getContactListDisplayName } from '../intl/contactList';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import {
  accountTriState,
  buildSidebarTree,
  flattenLeaves,
  LOCAL_ACCOUNT_ID,
  parentTriState,
  type AccountNode,
  type LeafNode,
  type SectionNode,
  type TriState,
} from '../state/sidebarTree';
import {
  canReparentList,
  createCapableAccounts,
  reparentCandidates,
} from '../state/taskMoves';
import { useSidebarExpansion } from '../state/useSidebarExpansion';
import { useTaskListShowCompleted } from '../state/useTaskListShowCompleted';
import { useViewState } from '../state/viewStateContext';

/** Pointer drag-and-drop reparent state, threaded to the task-list
 *  leaf rows. Keyboard users get the same outcome via the
 *  "Move under …" context-menu submenu — this is the mouse path. */
interface ReparentDrag {
  draggingListId: string | null;
  begin: (id: string) => void;
  end: () => void;
  /** Whether the dragged list may drop onto `targetId` as its parent. */
  canDropOn: (targetId: string) => boolean;
  /** Commit the drop. `null` target ⇒ promote to top level. */
  drop: (targetId: string | null) => void;
}

/**
 * Sidebar-local extension of `ContainerKind` to include contact
 * lists. The backend rename/override surface still only knows
 * `calendar` / `task_list`; contacts have their own dedicated
 * commands (`rename_contact_list`, `delete_contact_list`) and
 * bypass the override path entirely. Threading a distinct
 * `'contact_list'` discriminator through the editing state lets
 * `commitEdit` branch cleanly without smuggling a contacts code
 * path through the override-aware `renameContainer`.
 */
type LeafEditKind = ContainerKind | 'contact_list';

/**
 * Sidebar: tree view of accounts → sections (Calendars / Tasks) →
 * individual containers, with multi-select via space and a single
 * tab stop on the container (`aria-activedescendant`).
 *
 * Keyboard model (W3C ARIA APG, treeview multi-select variant):
 *
 *   - ↑/↓     move focus between visible items (skipping collapsed
 *             children)
 *   - ←       on an open parent: collapse; on a leaf or closed parent:
 *             move to parent
 *   - →       on a closed parent: expand; on an open parent: focus
 *             first child
 *   - Home    first visible item
 *   - End     last visible item
 *   - Space   toggle selection on the focused item. For a parent
 *             with a `mixed` or `checked` state this clears every
 *             descendant; for `unchecked` it selects every descendant.
 *   - Enter   alias for Space
 *   - a-z     type-ahead: focus the next visible item whose name
 *             starts with the typed letter
 *
 * The flat list of visible items is recomputed on every render from
 * the tree + expansion state. That's O(n) over a handful of dozen
 * items and saves the alternative book-keeping (DOM walks, refs to
 * every item) which would be brittler with React's render model.
 *
 * Rename, delete, and create are kept as inline actions on the leaf
 * rows — the structural change is purely in the tree wrapping.
 */
export function Sidebar() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const {
    accounts,
    calendars,
    selectedCalendarIds,
    toggleCalendar,
    refreshCalendars,
    taskLists,
    selectedTaskListIds,
    toggleTaskList,
    refreshTaskLists,
    contactLists,
    selectedContactListIds,
    toggleContactList,
    refreshContactLists,
    colorLabels,
  } = useCalendarStore();
  const { openSettings, openTaskMembers } = useDialogState();
  const expansion = useSidebarExpansion();
  const showCompleted = useTaskListShowCompleted();
  const { focusedCalendarId, enterFocus, exitFocus } = useViewState();
  const isFocused = focusedCalendarId !== null;

  const tree = useMemo(
    () =>
      buildSidebarTree({
        accounts,
        calendars,
        taskLists,
        contactLists,
        selectedCalendarIds,
        selectedTaskListIds,
        selectedContactListIds,
      }),
    [
      accounts,
      calendars,
      taskLists,
      contactLists,
      selectedCalendarIds,
      selectedTaskListIds,
      selectedContactListIds,
    ],
  );

  // Flatten the tree to the list the user can navigate through. Each
  // entry carries enough context to render itself and to handle a
  // toggle/expand/collapse action without re-walking the tree. Hidden
  // children (those of a collapsed parent) are omitted entirely.
  const visible = useMemo<VisibleItem[]>(
    () => flattenTree(tree, expansion.isExpanded),
    [tree, expansion],
  );

  // ── Active descendant + keyboard navigation ───────────────────────
  const treeId = useId();
  const itemId = useCallback(
    (key: string) => `${treeId}-node-${key}`,
    [treeId],
  );

  const [focusedKey, setFocusedKey] = useState<string | null>(null);

  // Once the tree first populates, focus the first item so arrow-key
  // navigation has a starting point. Don't override an existing
  // focused key (the user may have already moved around).
  useEffect(() => {
    if (focusedKey === null && visible.length > 0) {
      setFocusedKey(visible[0].key);
    }
    // If the focused row disappears (account deleted, section
    // collapsed), fall back to the closest survivor.
    if (focusedKey !== null && !visible.some((v) => v.key === focusedKey)) {
      setFocusedKey(visible[0]?.key ?? null);
    }
  }, [visible, focusedKey]);

  // ── Inline rename plumbing ────────────────────────────────────────
  // Identifies the leaf currently in edit mode (`null` = no edit).
  const [editing, setEditing] = useState<{
    kind: LeafEditKind;
    id: string;
  } | null>(null);
  const [draft, setDraft] = useState('');
  // Set after commit/cancel — kicks the next `useEffect` into
  // returning DOM focus to the tree container so the aria-active
  // descendant takes over from the unmounted input. Boolean rather
  // than the edit target itself: the tree owns the tab stop, all we
  // need is "should focus go back to the tree?".
  const [restoreFocusToTree, setRestoreFocusToTree] = useState(false);

  // Ref on the `<div role="tree">` so we can move actual DOM focus
  // back to it after the rename input unmounts or the context menu
  // closes. Without this, `aria-activedescendant` stays accurate
  // but the user loses keyboard control of the tree entirely.
  const treeRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!restoreFocusToTree) return;
    treeRef.current?.focus({ preventScroll: true });
    setRestoreFocusToTree(false);
  }, [restoreFocusToTree]);

  const startEdit = useCallback(
    (kind: LeafEditKind, id: string, currentName: string) => {
      setEditing({ kind, id });
      setDraft(currentName);
    },
    [],
  );

  const cancelEdit = useCallback(
    (restoreFocus: boolean) => {
      if (!editing) return;
      setEditing(null);
      setDraft('');
      if (restoreFocus) setRestoreFocusToTree(true);
    },
    [editing],
  );

  const commitEdit = useCallback(
    async (restoreFocus: boolean) => {
      if (!editing) return;
      const { kind, id } = editing;
      const trimmed = draft.trim();
      try {
        // Contact lists don't participate in the override system —
        // there's only `rename_contact_list`, which routes through
        // the registry the same way `renameContainer` does for
        // calendars / tasks. An empty draft is rejected: clearing
        // a name has no local-override fallback to land in.
        if (kind === 'contact_list') {
          if (trimmed === '') {
            announce(t('sidebar.contactListNameRequired'));
            return;
          }
          await renameContactList(id, trimmed);
          announce(t('sidebar.renamedSynced', { name: trimmed }));
          await refreshContactLists();
        } else if (trimmed === '') {
          await clearContainerNameOverride(id, kind);
          announce(t('sidebar.renameCleared'));
          if (kind === 'calendar') {
            await refreshCalendars();
          } else {
            await refreshTaskLists();
          }
        } else {
          const outcome = await renameContainer(id, kind, trimmed);
          announce(
            t(
              outcome.synced_to_source
                ? 'sidebar.renamedSynced'
                : 'sidebar.renamedLocalOnly',
              { name: trimmed },
            ),
          );
          if (kind === 'calendar') {
            await refreshCalendars();
          } else {
            await refreshTaskLists();
          }
        }
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      } finally {
        setEditing(null);
        setDraft('');
        if (restoreFocus) setRestoreFocusToTree(true);
      }
    },
    [
      editing,
      draft,
      refreshCalendars,
      refreshTaskLists,
      refreshContactLists,
      announce,
      t,
    ],
  );

  const onEditKey = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        void commitEdit(true);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        cancelEdit(true);
      }
    },
    [commitEdit, cancelEdit],
  );

  // ── Create / delete actions ──────────────────────────────────────
  // A pending "name your new list" prompt. The three create buttons no
  // longer mint an auto-name and create immediately — they open this
  // prompt (pre-filled with the old auto-name as a suggestion), and the
  // actual create runs on submit with whatever the user typed. For task
  // lists the account is resolved first (when more than one is capable).
  const [createPrompt, setCreatePrompt] = useState<
    | { kind: 'calendar' }
    | { kind: 'taskList'; accountId: string }
    | { kind: 'contactList' }
    | null
  >(null);

  const onCreateCalendar = useCallback(() => {
    setCreatePrompt({ kind: 'calendar' });
  }, []);

  const onDeleteCalendar = useCallback(
    async (id: string, name: string) => {
      try {
        await deleteCalendar(id);
        await refreshCalendars();
        announce(t('sidebar.calendarDeleted', { name }));
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('delete_calendar failed', err);
      }
    },
    [refreshCalendars, announce, t],
  );

  const onDeleteTaskListAction = useCallback(
    async (id: string, name: string) => {
      try {
        await deleteTaskList(id);
        await refreshTaskLists();
        announce(t('sidebar.taskListDeleted', { name }));
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('delete_task_list failed', err);
      }
    },
    [refreshTaskLists, announce, t],
  );

  // Reparent a local task list under another (or to the top level when
  // `newParentId` is null). The context-menu submenu and drag-and-drop
  // both funnel here; a final `canReparentList` guard catches a stale
  // tree (a concurrent sync could move a candidate out from under us).
  const reparentList = useCallback(
    async (childId: string, newParentId: string | null) => {
      if (
        newParentId !== null &&
        !canReparentList(childId, newParentId, taskLists)
      ) {
        return;
      }
      const child = taskLists.find((l) => l.id === childId);
      const parent = newParentId
        ? taskLists.find((l) => l.id === newParentId)
        : null;
      try {
        await reparentTaskList(childId, newParentId);
        await refreshTaskLists();
        announce(
          newParentId
            ? t('sidebar.menu.reparentedAnnouncement', {
                name: child?.name ?? childId,
                parent: parent?.name ?? newParentId,
              })
            : t('sidebar.menu.reparentedTopAnnouncement', {
                name: child?.name ?? childId,
              }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('reparent_task_list failed', err);
      }
    },
    [taskLists, refreshTaskLists, announce, t],
  );

  // Pointer drag-and-drop reparent. Only local task lists participate
  // (the backend command is local-store only); `canDropOn` enforces
  // the same capability / cycle / account rules the menu uses.
  const [draggingListId, setDraggingListId] = useState<string | null>(null);
  // Pending task-list deletion awaiting confirmation. Deleting a list
  // removes it + all its tasks; for external accounts that's an
  // irreversible server-side delete, so the dialog spells that out.
  const [pendingListDelete, setPendingListDelete] = useState<{
    id: string;
    name: string;
    accountName: string | null;
  } | null>(null);
  const reparentDrag = useMemo<ReparentDrag>(
    () => ({
      draggingListId,
      begin: (id) => setDraggingListId(id),
      end: () => setDraggingListId(null),
      canDropOn: (targetId) => {
        if (draggingListId === null || draggingListId === targetId) {
          return false;
        }
        const dragged = taskLists.find((l) => l.id === draggingListId);
        if (dragged?.account_id !== LOCAL_ACCOUNT_ID) return false;
        return canReparentList(draggingListId, targetId, taskLists);
      },
      drop: (targetId) => {
        const dragged = draggingListId;
        setDraggingListId(null);
        if (dragged) void reparentList(dragged, targetId);
      },
    }),
    [draggingListId, taskLists, reparentList],
  );

  // Accounts a new list can be created in: the local store (always) +
  // every external account whose lists declare `create_lists`. Caps are
  // uniform per account, so any one of its lists answers for it.
  const capableAccounts = useMemo(
    () =>
      createCapableAccounts(
        taskLists,
        accounts,
        LOCAL_ACCOUNT_ID,
        t('sidebar.localAccount'),
      ),
    [taskLists, accounts, t],
  );

  const onCreateTaskList = useCallback(async () => {
    // One capable account (just local) → prompt straight away. Several →
    // a native picker (keyboard + mouse) for "which account?" first, then
    // the name prompt.
    if (capableAccounts.length <= 1) {
      setCreatePrompt({ kind: 'taskList', accountId: LOCAL_ACCOUNT_ID });
      return;
    }
    const items: ContextMenuItemRequest[] = capableAccounts.map((a) => ({
      id: `acct:${a.id}`,
      label: a.name,
    }));
    let selected: string | null = null;
    try {
      selected = await showContextMenu(items);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('show_context_menu failed', err);
    }
    if (selected?.startsWith('acct:')) {
      setCreatePrompt({
        kind: 'taskList',
        accountId: selected.slice('acct:'.length),
      });
    }
  }, [capableAccounts]);

  const onCreateContactList = useCallback(() => {
    setCreatePrompt({ kind: 'contactList' });
  }, []);

  // Run the actual create once the user has named the list in the prompt.
  const submitCreatePrompt = useCallback(
    async (name: string, colorLabelId?: string | null) => {
      const prompt = createPrompt;
      if (!prompt) return;
      // Container colors come from the SAME predefined color-labels as
      // everything else: resolve the picked label to its hex (or null for
      // "no color"). Keeps one palette across tasks, events + containers.
      const colorHex = colorLabelId
        ? (colorLabels.find((l) => l.id === colorLabelId)?.hex ?? null)
        : null;
      try {
        if (prompt.kind === 'calendar') {
          const cal = await createCalendar({ name, color_hex: colorHex });
          await refreshCalendars();
          announce(t('sidebar.calendarCreated', { name: cal.name }));
        } else if (prompt.kind === 'taskList') {
          const list = await createTaskList({
            name,
            account_id:
              prompt.accountId === LOCAL_ACCOUNT_ID ? null : prompt.accountId,
            parent_id: null,
            embedded_in_calendar: null,
          });
          await refreshTaskLists();
          announce(t('sidebar.taskListCreated', { name: list.name }));
        } else {
          const list = await createContactList({ name, color_hex: colorHex });
          await refreshContactLists();
          announce(t('sidebar.contactListCreated', { name: list.name }));
        }
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('create list failed', err);
      }
    },
    [
      createPrompt,
      colorLabels,
      refreshCalendars,
      refreshTaskLists,
      refreshContactLists,
      announce,
      t,
    ],
  );

  const onDeleteContactListAction = useCallback(
    async (id: string, name: string) => {
      try {
        await deleteContactList(id);
        await refreshContactLists();
        announce(t('sidebar.contactListDeleted', { name }));
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          // eslint-disable-next-line no-console
          console.warn('delete_contact_list failed', err);
        }
      }
    },
    [refreshContactLists, announce, t],
  );

  // ── Context menu plumbing ────────────────────────────────────────
  //
  // Pops the OS-native context menu via Tauri's `tauri::menu::Menu`
  // popup — real Win32 / NSMenu / GTK menus that the platform's a11y
  // layer already knows about. Two triggers: right-click on a leaf
  // (mouse position implied), and Shift+F10 / ContextMenu key on the
  // focused row (window-logical position passed explicitly).
  //
  // The native menu eats keyboard focus while open and returns it to
  // our window on close. We then bounce focus back to the tree
  // container so `aria-activedescendant` takes the lead again.
  const openContextMenuForLeaf = useCallback(
    async (
      leaf: LeafNode,
      accountIsLocal: boolean,
      position?: { x: number; y: number },
    ) => {
      // Two kinds end up writing back through the override system
      // (`calendar` / `task_list`); contacts have their own
      // `rename_contact_list` write path. The `editKind` value the
      // rest of this function passes around is the
      // sidebar-local `LeafEditKind` so the editing-state
      // discriminator and the rename router agree.
      const editKind: LeafEditKind =
        leaf.kind === 'calendars'
          ? 'calendar'
          : leaf.kind === 'tasks'
            ? 'task_list'
            : 'contact_list';
      const items: ContextMenuItemRequest[] = [
        { id: 'rename', label: t('sidebar.menu.rename') },
      ];
      // Focus-mode toggle: only meaningful on calendar leaves (tasks
      // don't participate in the focus drill-in today). When we're
      // already focused on a different calendar, "Show only this
      // calendar" still works — it just switches the focused id.
      if (leaf.kind === 'calendars') {
        if (focusedCalendarId === leaf.containerId) {
          items.push({
            id: 'focus-exit',
            label: t('sidebar.menu.focusExit'),
          });
        } else {
          items.push({
            id: 'focus-open',
            label: t('sidebar.menu.focusOpen'),
          });
        }
      }
      // Task-list-only setting: whether completed tasks stay visible
      // in the calendar surfaces (WeekView, DayView). Built as a
      // native CheckMenuItem so the OS draws its own check-mark
      // glyph — the user reads the state at a glance instead of
      // parsing "show vs hide" wording.
      //
      // `kind: 'check'` is required: when the chip-context-menu
      // refactor (Phase #92) turned the request shape into a
      // discriminated union, the default kind became `text`, which
      // would silently strip the `checked` field and render this
      // entry as a plain row — the toggle would still fire on click,
      // but the user would never see whether the option was on.
      if (leaf.kind === 'tasks') {
        items.push({
          kind: 'check',
          id: 'toggle-show-completed',
          label: t('sidebar.menu.showCompletedInCalendar'),
          checked: showCompleted.shouldShow(leaf.containerId),
        });
        // "Move under …" — reparent a local project. Local-only:
        // external-provider projects are nested in their own UI.
        // The submenu lists every valid target (reparentCandidates
        // already drops self / current-parent / descendants / other
        // accounts) plus a "top level" entry when the list is nested.
        if (accountIsLocal) {
          const currentList = taskLists.find((l) => l.id === leaf.containerId);
          const subItems: ContextMenuItemRequest[] = [];
          if (currentList?.parent_id) {
            subItems.push({
              id: 'reparent:__top__',
              label: t('sidebar.menu.moveToTopLevel'),
            });
          }
          for (const cand of reparentCandidates(leaf.containerId, taskLists)) {
            subItems.push({ id: `reparent:${cand.id}`, label: cand.name });
          }
          if (subItems.length > 0) {
            items.push({
              id: 'reparent',
              label: t('sidebar.menu.moveUnder'),
              items: subItems,
            });
          }
        }
        // Manage who the list is shared with — only where the adapter
        // declares membership management (Vikunja, Todoist). Local lists
        // and flat backends (Google Tasks, MS To Do) have no membership
        // concept, so they never offer the entry.
        const membersCaps = taskLists.find(
          (l) => l.id === leaf.containerId,
        )?.task_capabilities;
        if (membersCaps?.manageable) {
          items.push({ id: 'members', label: t('sidebar.menu.members') });
        }
      }
      // Delete: always for local containers; for external task lists
      // only when the adapter declares `delete_lists` (Vikunja etc.).
      // Other external sources (calendars, contacts) still have no
      // one-click delete affordance — rename is the common case there.
      const externalListDeletable =
        leaf.kind === 'tasks' &&
        !accountIsLocal &&
        (taskLists.find((l) => l.id === leaf.containerId)?.task_capabilities
          ?.delete_lists ??
          false);
      if (accountIsLocal || externalListDeletable) {
        items.push({ id: 'delete', label: t('sidebar.menu.delete') });
      }

      let selected: string | null = null;
      try {
        selected = await showContextMenu(items, position);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('show_context_menu failed', err);
      }

      if (selected === 'rename') {
        // Don't steer focus back to the tree here — startEdit() swaps
        // the leaf row's body into an <input autoFocus />, which grabs
        // focus during its mount. If we also ran the tree-restore
        // effect, it would race the autoFocus and silently win, leaving
        // the user staring at a rename input they can't type into.
        startEdit(editKind, leaf.containerId, leaf.name);
      } else if (selected === 'members') {
        setRestoreFocusToTree(true);
        const caps = taskLists.find(
          (l) => l.id === leaf.containerId,
        )?.task_capabilities;
        openTaskMembers(leaf.containerId, leaf.name, caps);
      } else if (selected === 'focus-open') {
        setRestoreFocusToTree(true);
        enterFocus(leaf.containerId);
        announce(
          t('sidebar.focus.enteredAnnouncement', { name: leaf.name }),
        );
      } else if (selected === 'focus-exit') {
        setRestoreFocusToTree(true);
        exitFocus();
        announce(t('sidebar.focus.exitedAnnouncement'));
      } else if (selected === 'toggle-show-completed') {
        // Per-list preference (Phase 9.4 follow-up). The hook
        // persists via user_prefs; WeekView and DayView observe the
        // same store, so the calendar grids refresh on the next
        // render once the toggle lands.
        setRestoreFocusToTree(true);
        const wasShowing = showCompleted.shouldShow(leaf.containerId);
        showCompleted.toggle(leaf.containerId);
        announce(
          wasShowing
            ? t('sidebar.menu.hideCompletedAnnouncement', {
                name: leaf.name,
              })
            : t('sidebar.menu.showCompletedAnnouncement', {
                name: leaf.name,
              }),
        );
      } else if (selected === 'delete') {
        if (leaf.kind === 'tasks') {
          // Confirm first — deleting a list takes all its tasks with it,
          // and for external accounts that's irreversible at the source.
          // The dialog grabs focus, so don't restore tree-focus here.
          let accountName: string | null = null;
          if (!accountIsLocal) {
            const list = taskLists.find((l) => l.id === leaf.containerId);
            accountName =
              accounts.find((a) => a.id === list?.account_id)?.display_name ??
              null;
          }
          setPendingListDelete({
            id: leaf.containerId,
            name: leaf.name,
            accountName,
          });
        } else {
          // Calendars / contacts: existing direct delete. The row
          // vanishes, so tree-focus is where keyboard nav should land.
          setRestoreFocusToTree(true);
          if (leaf.kind === 'calendars') {
            void onDeleteCalendar(leaf.containerId, leaf.name);
          } else {
            void onDeleteContactListAction(leaf.containerId, leaf.name);
          }
        }
      } else if (selected?.startsWith('reparent:')) {
        setRestoreFocusToTree(true);
        const target = selected.slice('reparent:'.length);
        void reparentList(leaf.containerId, target === '__top__' ? null : target);
      } else {
        // Menu dismissed (Escape, click-away) — no action; just hand
        // keyboard control back to the tree.
        setRestoreFocusToTree(true);
      }
    },
    [
      startEdit,
      openTaskMembers,
      onDeleteCalendar,
      onDeleteContactListAction,
      taskLists,
      accounts,
      reparentList,
      t,
      focusedCalendarId,
      enterFocus,
      exitFocus,
      announce,
      showCompleted,
    ],
  );

  // Helper: find the leaf for a given visible item key. The
  // keyboard-triggered menu uses this to know what to populate.
  const findLeafByKey = useCallback(
    (key: string): { leaf: LeafNode; accountIsLocal: boolean } | null => {
      for (const account of tree) {
        const isLocal = account.accountId === 'local';
        for (const section of account.children) {
          for (const leaf of section.children) {
            if (leaf.key === key) return { leaf, accountIsLocal: isLocal };
          }
        }
      }
      return null;
    },
    [tree],
  );

  // ── Multi-toggle: flip every descendant of a parent on/off ───────
  const setManyCalendars = useCallback(
    (ids: string[], next: boolean) => {
      // Toggle reaches into the store one id at a time — the store
      // batches the state-update internally because each toggle is
      // a setState callback.
      for (const id of ids) {
        const isOn = selectedCalendarIds.has(id);
        if (isOn !== next) toggleCalendar(id);
      }
    },
    [selectedCalendarIds, toggleCalendar],
  );
  const setManyTaskLists = useCallback(
    (ids: string[], next: boolean) => {
      for (const id of ids) {
        const isOn = selectedTaskListIds.has(id);
        if (isOn !== next) toggleTaskList(id);
      }
    },
    [selectedTaskListIds, toggleTaskList],
  );
  const setManyContactLists = useCallback(
    (ids: string[], next: boolean) => {
      for (const id of ids) {
        const isOn = selectedContactListIds.has(id);
        if (isOn !== next) toggleContactList(id);
      }
    },
    [selectedContactListIds, toggleContactList],
  );

  // ── Keyboard handler on the tree container ───────────────────────
  // Type-ahead buffer: collected single-character keypresses within
  // a 700ms window become a prefix search.
  const typeBuffer = useRef('');
  const typeTimer = useRef<number | null>(null);

  const focusByIndex = useCallback(
    (idx: number) => {
      if (idx < 0 || idx >= visible.length) return;
      setFocusedKey(visible[idx].key);
    },
    [visible],
  );

  const onTreeKey = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (focusedKey === null || visible.length === 0) return;
      const idx = visible.findIndex((v) => v.key === focusedKey);
      if (idx < 0) return;
      const item = visible[idx];

      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          focusByIndex(Math.min(idx + 1, visible.length - 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          focusByIndex(Math.max(idx - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          focusByIndex(0);
          return;
        case 'End':
          e.preventDefault();
          focusByIndex(visible.length - 1);
          return;
        case 'ArrowRight': {
          e.preventDefault();
          // Leaves are expandable too when they nest child projects.
          const expandable = item.kind !== 'leaf' || item.hasChildren;
          if (!expandable) return;
          if (expansion.isExpanded(item.key)) {
            // Already open → move to first child if any.
            const first = visible.find(
              (v, i) => i > idx && v.parentKey === item.key,
            );
            if (first) setFocusedKey(first.key);
          } else {
            expansion.setExpanded(item.key, true);
          }
          return;
        }
        case 'ArrowLeft': {
          e.preventDefault();
          const expandable = item.kind !== 'leaf' || item.hasChildren;
          if (expandable && expansion.isExpanded(item.key)) {
            expansion.setExpanded(item.key, false);
          } else if (item.parentKey) {
            setFocusedKey(item.parentKey);
          }
          return;
        }
        case ' ':
        case 'Spacebar':
        case 'Enter':
          e.preventDefault();
          toggleItem(item);
          return;
        case 'F10':
          // Shift+F10 is the keyboard-canonical context-menu trigger
          // on every desktop OS. Without Shift, F10 falls through (it
          // moves focus to the OS menu bar on Windows and we don't
          // want to swallow that).
          if (e.shiftKey && item.kind === 'leaf') {
            e.preventDefault();
            const target = e.currentTarget.querySelector(
              `#${CSS.escape(itemId(item.key))}`,
            );
            const rect =
              target instanceof HTMLElement
                ? target.getBoundingClientRect()
                : null;
            const ctx = findLeafByKey(item.key);
            if (ctx && rect) {
              void openContextMenuForLeaf(ctx.leaf, ctx.accountIsLocal, {
                x: rect.left + 8,
                y: rect.bottom,
              });
            }
            return;
          }
          break;
        case 'ContextMenu': {
          // The Menu / ContextMenu key on Windows keyboards. Some
          // setups also surface it as Shift+F10 above; covering both
          // here is cheap.
          if (item.kind !== 'leaf') return;
          e.preventDefault();
          const target = e.currentTarget.querySelector(
            `#${CSS.escape(itemId(item.key))}`,
          );
          const rect =
            target instanceof HTMLElement
              ? target.getBoundingClientRect()
              : null;
          const ctx = findLeafByKey(item.key);
          if (ctx && rect) {
            void openContextMenuForLeaf(ctx.leaf, ctx.accountIsLocal, {
              x: rect.left + 8,
              y: rect.bottom,
            });
          }
          return;
        }
      }

      // Type-ahead: collect printable single chars (no modifiers
      // beyond shift), reset on 700ms idle, jump to the next item
      // whose name starts with the buffer.
      if (
        e.key.length === 1 &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        /\S/.test(e.key)
      ) {
        typeBuffer.current = (typeBuffer.current + e.key).toLowerCase();
        if (typeTimer.current !== null) {
          window.clearTimeout(typeTimer.current);
        }
        typeTimer.current = window.setTimeout(() => {
          typeBuffer.current = '';
          typeTimer.current = null;
        }, 700);
        // Search from the next item, wrapping around. This matches
        // the "press 'c' twice to skip past the first c-row" pattern
        // most file managers use.
        const buf = typeBuffer.current;
        const ring = [...visible.slice(idx + 1), ...visible.slice(0, idx + 1)];
        const hit = ring.find((v) =>
          v.label.toLowerCase().startsWith(buf),
        );
        if (hit) setFocusedKey(hit.key);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      focusedKey,
      visible,
      expansion,
      focusByIndex,
      findLeafByKey,
      openContextMenuForLeaf,
      itemId,
    ],
  );

  // Toggle for both leaves (simple selection) and parents (apply to
  // all descendants).
  const toggleItem = useCallback(
    (item: VisibleItem) => {
      if (item.kind === 'leaf') {
        if (item.sectionKind === 'calendars') {
          toggleCalendar(item.containerId);
        } else if (item.sectionKind === 'tasks') {
          toggleTaskList(item.containerId);
        } else {
          toggleContactList(item.containerId);
        }
        return;
      }
      // Parent: collect the leaves and flip them as a batch.
      const leaves = collectLeaves(item, tree);
      const state = leaves.every((l) => l.selected)
        ? 'checked'
        : leaves.some((l) => l.selected)
          ? 'mixed'
          : 'unchecked';
      // mixed / checked → turn everything off; unchecked → on.
      const next = state === 'unchecked';
      const calIds = leaves
        .filter((l) => l.kind === 'calendars')
        .map((l) => l.containerId);
      const tlIds = leaves
        .filter((l) => l.kind === 'tasks')
        .map((l) => l.containerId);
      const clIds = leaves
        .filter((l) => l.kind === 'contacts')
        .map((l) => l.containerId);
      if (calIds.length) setManyCalendars(calIds, next);
      if (tlIds.length) setManyTaskLists(tlIds, next);
      if (clIds.length) setManyContactLists(clIds, next);
    },
    [
      tree,
      toggleCalendar,
      toggleTaskList,
      toggleContactList,
      setManyCalendars,
      setManyTaskLists,
      setManyContactLists,
    ],
  );

  // ── Render ───────────────────────────────────────────────────────
  return (
    <aside
      className="sidebar"
      aria-label={t('sidebar.label')}
      data-region="sidebar"
    >
      <h2 className="sidebar__heading">{t('sidebar.containersHeading')}</h2>
      {/* SR-only hint that describes the tree's interaction model.
          Surfaces on focus so the user learns the context-menu
          trigger without it cluttering the visual UI. */}
      <p id={`${treeId}-hint`} className="sr-only">
        {t('sidebar.tree.contextMenuHint')}
      </p>
      {/* The tree itself owns the tab stop. aria-activedescendant
          tells assistive tech which row is "focused" inside the
          tree without React having to focus DOM nodes individually. */}
      <div
        ref={treeRef}
        role="tree"
        aria-label={t('sidebar.treeLabel')}
        aria-describedby={`${treeId}-hint`}
        aria-multiselectable="true"
        tabIndex={0}
        onKeyDown={onTreeKey}
        aria-activedescendant={
          focusedKey ? itemId(focusedKey) : undefined
        }
        onContextMenu={(e) => {
          // Walk up from the click target to find the originating
          // treeitem element; we keyed it with `itemId(...)`. If
          // the click missed every leaf (clicked the padding around
          // a row), fall through to the OS context menu — there's
          // nothing actionable here.
          const target = e.target as HTMLElement;
          const item = target.closest('[role="treeitem"]');
          if (!(item instanceof HTMLElement)) return;
          // Extract the visible-item key from the DOM id. The shape
          // is `${treeId}-node-${key}` so we just strip the prefix.
          const prefix = `${treeId}-node-`;
          if (!item.id.startsWith(prefix)) return;
          const key = item.id.slice(prefix.length);
          const ctx = findLeafByKey(key);
          if (!ctx) return; // not a leaf row — skip the menu
          e.preventDefault();
          setFocusedKey(key);
          // Omit `position`: the OS anchors at the cursor by default,
          // which handles multi-monitor / RTL edge cases for us.
          void openContextMenuForLeaf(ctx.leaf, ctx.accountIsLocal);
        }}
        className={
          'sidebar__tree' +
          (isFocused ? ' sidebar__tree--in-focus-mode' : '')
        }
      >
        {tree.map((account) => (
          <AccountSubtree
            key={account.key}
            account={account}
            expansion={expansion}
            focusedKey={focusedKey}
            onFocusKey={setFocusedKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={setDraft}
            onCancelEdit={cancelEdit}
            onCommitEdit={commitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={(leaf) => {
              if (leaf.kind === 'calendars') {
                toggleCalendar(leaf.containerId);
              } else if (leaf.kind === 'tasks') {
                toggleTaskList(leaf.containerId);
              } else {
                toggleContactList(leaf.containerId);
              }
            }}
            onToggleSection={(section) => {
              const leaves = flattenLeaves(section.children);
              const state = parentTriState(leaves);
              const next = state === 'unchecked';
              const ids = leaves.map((l) => l.containerId);
              if (section.kind === 'calendars') {
                setManyCalendars(ids, next);
              } else if (section.kind === 'tasks') {
                setManyTaskLists(ids, next);
              } else {
                setManyContactLists(ids, next);
              }
            }}
            onToggleAccount={(node) => {
              const leaves = flattenLeaves(
                node.children.flatMap((s) => s.children),
              );
              const state = accountTriState(node);
              const next = state === 'unchecked';
              const calIds = leaves
                .filter((l) => l.kind === 'calendars')
                .map((l) => l.containerId);
              const tlIds = leaves
                .filter((l) => l.kind === 'tasks')
                .map((l) => l.containerId);
              const clIds = leaves
                .filter((l) => l.kind === 'contacts')
                .map((l) => l.containerId);
              if (calIds.length) setManyCalendars(calIds, next);
              if (tlIds.length) setManyTaskLists(tlIds, next);
              if (clIds.length) setManyContactLists(clIds, next);
            }}
            focusedContainerId={focusedCalendarId}
            reparentDrag={reparentDrag}
          />
        ))}
      </div>

      <div className="sidebar__add-row">
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateCalendar}
        >
          + {t('sidebar.newCalendar')}
        </button>
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateTaskList}
        >
          + {t('sidebar.newTaskList')}
        </button>
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateContactList}
        >
          + {t('sidebar.newContactList')}
        </button>
      </div>

      <section className="sidebar__section">
        <button
          type="button"
          className="sidebar__add"
          onClick={() => openSettings()}
        >
          {t('sidebar.openSettings')}
        </button>
      </section>

      <ConfirmDialog
        isOpen={pendingListDelete !== null}
        onClose={() => {
          setPendingListDelete(null);
          setRestoreFocusToTree(true);
        }}
        onConfirm={() => {
          if (pendingListDelete) {
            void onDeleteTaskListAction(
              pendingListDelete.id,
              pendingListDelete.name,
            );
          }
          setPendingListDelete(null);
          setRestoreFocusToTree(true);
        }}
        title={t('dialogs.confirm.deleteTaskListTitle')}
        message={
          pendingListDelete?.accountName
            ? t('dialogs.confirm.deleteTaskListMessageExternal', {
                name: pendingListDelete.name,
                account: pendingListDelete.accountName,
              })
            : t('dialogs.confirm.deleteTaskListMessage', {
                name: pendingListDelete?.name ?? '',
              })
        }
      />
      <PromptDialog
        isOpen={createPrompt !== null}
        onClose={() => setCreatePrompt(null)}
        onSubmit={submitCreatePrompt}
        title={
          createPrompt?.kind === 'taskList'
            ? t('sidebar.newTaskList')
            : createPrompt?.kind === 'contactList'
              ? t('sidebar.newContactList')
              : t('sidebar.newCalendar')
        }
        label={t('sidebar.createPrompt.nameLabel')}
        defaultValue={
          createPrompt?.kind === 'taskList'
            ? t('sidebar.newTaskListName', { n: taskLists.length + 1 })
            : createPrompt?.kind === 'contactList'
              ? t('sidebar.newContactListName', { n: contactLists.length + 1 })
              : t('sidebar.newCalendarName', { n: calendars.length + 1 })
        }
        submitLabel={t('sidebar.createPrompt.submit')}
        colorField={
          // Task-list creation doesn't carry a color through the wire
          // yet; calendars + address books do, so only they get the
          // color picker. The color is chosen from the predefined
          // color-labels (same palette as tasks/events), not a free hex.
          createPrompt?.kind === 'calendar' ||
          createPrompt?.kind === 'contactList'
            ? {
                label: t('sidebar.createPrompt.colorLabel'),
                labels: colorLabels,
                noneLabel: t('sidebar.createPrompt.noColor'),
                defaultLabelId: null,
              }
            : undefined
        }
      />
    </aside>
  );
}

// ────────────────────────────────────────────────────────────────────────
// Flattened-tree model used for keyboard navigation
// ────────────────────────────────────────────────────────────────────────

type VisibleItem =
  | {
      kind: 'account' | 'section';
      key: string;
      label: string;
      parentKey: string | null;
      level: 1 | 2;
      sectionKind?: undefined;
      containerId?: undefined;
    }
  | {
      kind: 'leaf';
      key: string;
      label: string;
      parentKey: string;
      /** 3 for a section's direct child; +1 per nesting level. */
      level: number;
      sectionKind: 'calendars' | 'tasks' | 'contacts';
      containerId: string;
      /** True when this leaf nests child projects (Vikunja / Todoist) —
       *  drives the twisty + ArrowLeft/Right expand semantics. */
      hasChildren: boolean;
    };

function flattenTree(
  tree: AccountNode[],
  isExpanded: (key: string) => boolean,
): VisibleItem[] {
  const out: VisibleItem[] = [];
  const pushLeaf = (leaf: LeafNode, parentKey: string, level: number) => {
    out.push({
      kind: 'leaf',
      key: leaf.key,
      label: leaf.name,
      parentKey,
      level,
      sectionKind: leaf.kind,
      containerId: leaf.containerId,
      hasChildren: leaf.children.length > 0,
    });
    if (leaf.children.length > 0 && isExpanded(leaf.key)) {
      for (const child of leaf.children) pushLeaf(child, leaf.key, level + 1);
    }
  };
  for (const account of tree) {
    out.push({
      kind: 'account',
      key: account.key,
      label: account.displayName,
      parentKey: null,
      level: 1,
    });
    if (!isExpanded(account.key)) continue;
    for (const section of account.children) {
      out.push({
        kind: 'section',
        key: section.key,
        // Label uses the section's own label key; the real
        // localised string is resolved at render time.
        label: section.labelKey,
        parentKey: account.key,
        level: 2,
      });
      if (!isExpanded(section.key)) continue;
      for (const leaf of section.children) pushLeaf(leaf, section.key, 3);
    }
  }
  return out;
}

function collectLeaves(
  parent: VisibleItem,
  tree: AccountNode[],
): LeafNode[] {
  if (parent.kind === 'leaf') return [];
  // Locate the originating account / section node in the tree by key.
  // `flattenLeaves` pulls in nested task-list descendants so a section
  // or account toggle cascades through the whole project subtree.
  for (const account of tree) {
    if (account.key === parent.key) {
      return flattenLeaves(account.children.flatMap((s) => s.children));
    }
    for (const section of account.children) {
      if (section.key === parent.key) {
        return flattenLeaves(section.children);
      }
    }
  }
  return [];
}

// ────────────────────────────────────────────────────────────────────────
// Subtree rendering
// ────────────────────────────────────────────────────────────────────────

interface AccountSubtreeProps {
  account: AccountNode;
  expansion: ReturnType<typeof useSidebarExpansion>;
  focusedKey: string | null;
  onFocusKey: (key: string) => void;
  itemId: (key: string) => string;
  editing: { kind: LeafEditKind; id: string } | null;
  draft: string;
  onDraftChange: (v: string) => void;
  onCancelEdit: (restoreFocus: boolean) => void;
  onCommitEdit: (restoreFocus: boolean) => Promise<void>;
  onEditKey: (e: KeyboardEvent<HTMLInputElement>) => void;
  onToggleLeaf: (leaf: LeafNode) => void;
  onToggleSection: (section: SectionNode) => void;
  onToggleAccount: (account: AccountNode) => void;
  focusedContainerId: string | null;
  reparentDrag: ReparentDrag;
}

function AccountSubtree({
  account,
  expansion,
  focusedKey,
  onFocusKey,
  itemId,
  editing,
  draft,
  onDraftChange,
  onCancelEdit,
  onCommitEdit,
  onEditKey,
  onToggleLeaf,
  onToggleSection,
  onToggleAccount,
  focusedContainerId,
  reparentDrag,
}: AccountSubtreeProps) {
  const { t } = useTranslation();
  const isOpen = expansion.isExpanded(account.key);
  const state = accountTriState(account);
  // Reparent is a local-store gesture — only local accounts' task
  // lists are draggable / droppable.
  const accountIsLocal = account.accountId === LOCAL_ACCOUNT_ID;

  return (
    <div role="group" className="sidebar__account-group">
      <TreeRow
        id={itemId(account.key)}
        level={1}
        expanded={account.children.length > 0 ? isOpen : undefined}
        ariaChecked={triStateToAria(state)}
        focused={focusedKey === account.key}
        onPointerSelect={() => onFocusKey(account.key)}
        onToggleSelect={() => onToggleAccount(account)}
        onToggleExpand={() =>
          expansion.setExpanded(account.key, !isOpen)
        }
        ariaLabel={t('sidebar.tree.accountLabel', {
          name: account.displayName,
          kind: t(`dialogs.accounts.kindName.${account.adapterKind}`),
        })}
        className="sidebar__row sidebar__row--account"
      >
        <span className="sidebar__chevron" aria-hidden="true">
          {account.children.length === 0 ? '·' : isOpen ? '▾' : '▸'}
        </span>
        <span className="sidebar__name">{account.displayName}</span>
        <span className="sidebar__account-kind" aria-hidden="true">
          {t(`dialogs.accounts.kindName.${account.adapterKind}`)}
        </span>
      </TreeRow>
      {account.isEmpty && isOpen && (
        <div role="presentation" className="sidebar__empty-hint">
          {t('sidebar.tree.empty')}
        </div>
      )}
      {isOpen &&
        account.children.map((section) => (
          <SectionSubtree
            key={section.key}
            section={section}
            expansion={expansion}
            focusedKey={focusedKey}
            onFocusKey={onFocusKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={onDraftChange}
            onCancelEdit={onCancelEdit}
            onCommitEdit={onCommitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={onToggleLeaf}
            onToggleSection={onToggleSection}
            focusedContainerId={focusedContainerId}
            reparentDrag={reparentDrag}
            accountIsLocal={accountIsLocal}
          />
        ))}
    </div>
  );
}

interface SectionSubtreeProps {
  section: SectionNode;
  expansion: ReturnType<typeof useSidebarExpansion>;
  focusedKey: string | null;
  onFocusKey: (key: string) => void;
  itemId: (key: string) => string;
  editing: { kind: LeafEditKind; id: string } | null;
  draft: string;
  onDraftChange: (v: string) => void;
  onCancelEdit: (restoreFocus: boolean) => void;
  onCommitEdit: (restoreFocus: boolean) => Promise<void>;
  onEditKey: (e: KeyboardEvent<HTMLInputElement>) => void;
  onToggleLeaf: (leaf: LeafNode) => void;
  onToggleSection: (section: SectionNode) => void;
  focusedContainerId: string | null;
  reparentDrag: ReparentDrag;
  accountIsLocal: boolean;
}

function SectionSubtree({
  section,
  expansion,
  focusedKey,
  onFocusKey,
  itemId,
  editing,
  draft,
  onDraftChange,
  onCancelEdit,
  onCommitEdit,
  onEditKey,
  onToggleLeaf,
  onToggleSection,
  focusedContainerId,
  reparentDrag,
  accountIsLocal,
}: SectionSubtreeProps) {
  const { t } = useTranslation();
  const isOpen = expansion.isExpanded(section.key);
  const state = parentTriState(section.children);
  const sectionLabel =
    section.labelKey === 'calendars'
      ? t('sidebar.calendars')
      : section.labelKey === 'tasks'
        ? t('sidebar.taskLists')
        : t('sidebar.contactLists');

  return (
    <div role="group" className="sidebar__section-group">
      <TreeRow
        id={itemId(section.key)}
        level={2}
        expanded={section.children.length > 0 ? isOpen : undefined}
        ariaChecked={triStateToAria(state)}
        focused={focusedKey === section.key}
        onPointerSelect={() => onFocusKey(section.key)}
        onToggleSelect={() => onToggleSection(section)}
        onToggleExpand={() =>
          expansion.setExpanded(section.key, !isOpen)
        }
        ariaLabel={t('sidebar.tree.sectionLabel', { name: sectionLabel })}
        className="sidebar__row sidebar__row--section"
      >
        <span className="sidebar__chevron" aria-hidden="true">
          {section.children.length === 0 ? '·' : isOpen ? '▾' : '▸'}
        </span>
        <span className="sidebar__name">{sectionLabel}</span>
      </TreeRow>
      {isOpen &&
        section.children.map((leaf) => (
          <LeafRow
            key={leaf.key}
            leaf={leaf}
            depth={0}
            expansion={expansion}
            focusedKey={focusedKey}
            onFocusKey={onFocusKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={onDraftChange}
            onCancelEdit={onCancelEdit}
            onCommitEdit={onCommitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={onToggleLeaf}
            focusedContainerId={focusedContainerId}
            reparentDrag={reparentDrag}
            accountIsLocal={accountIsLocal}
          />
        ))}
    </div>
  );
}

interface LeafRowProps {
  leaf: LeafNode;
  /** Nesting depth within the section: 0 for a direct child, +1 per
   *  level. Drives ARIA level (3 + depth) and the indentation. */
  depth: number;
  expansion: ReturnType<typeof useSidebarExpansion>;
  focusedKey: string | null;
  onFocusKey: (key: string) => void;
  itemId: (key: string) => string;
  editing: { kind: LeafEditKind; id: string } | null;
  draft: string;
  onDraftChange: (v: string) => void;
  onCancelEdit: (restoreFocus: boolean) => void;
  onCommitEdit: (restoreFocus: boolean) => Promise<void>;
  onEditKey: (e: KeyboardEvent<HTMLInputElement>) => void;
  onToggleLeaf: (leaf: LeafNode) => void;
  /** Container id of the calendar currently in focus-mode (null when
   *  no focus is active). Used to mark the focused leaf visually so
   *  the user can see at a glance which row drives the main view. */
  focusedContainerId: string | null;
  reparentDrag: ReparentDrag;
  accountIsLocal: boolean;
}

function LeafRow({
  leaf,
  depth,
  expansion,
  focusedKey,
  onFocusKey,
  itemId,
  editing,
  draft,
  onDraftChange,
  onCancelEdit,
  onCommitEdit,
  onEditKey,
  onToggleLeaf,
  focusedContainerId,
  reparentDrag,
  accountIsLocal,
}: LeafRowProps) {
  const { t } = useTranslation();
  const [dropActive, setDropActive] = useState(false);
  // Only local task-list rows take part in reparent drag-and-drop.
  const dndEnabled = accountIsLocal && leaf.kind === 'tasks';
  // LeafEditKind discriminates the three leaf types so the rename
  // commit path can branch on the appropriate write API. Contact
  // leaves don't participate in the override system; the
  // `commitEdit` callback in Sidebar handles them separately.
  const kind: LeafEditKind =
    leaf.kind === 'calendars'
      ? 'calendar'
      : leaf.kind === 'tasks'
        ? 'task_list'
        : 'contact_list';
  // Synthetic system-managed lists (the EWS GAL today) carry an
  // English label from the backend; the helper swaps it for the
  // localized string when the sentinel id is recognised, falling
  // back to leaf.name verbatim otherwise.
  const displayName =
    leaf.kind === 'contacts'
      ? getContactListDisplayName(
          { id: leaf.containerId, name: leaf.name },
          t,
        )
      : leaf.name;
  const isEditing = editing?.kind === kind && editing.id === leaf.containerId;
  const isFocusTarget =
    leaf.kind === 'calendars' && leaf.containerId === focusedContainerId;
  // Nested task-list projects (Vikunja / Todoist) are both selectable
  // (own checkbox) and expandable (twisty). Flat lists have no children
  // and render exactly as before.
  const hasChildren = leaf.children.length > 0;
  const isOpen = expansion.isExpanded(leaf.key);

  const dnd = dndEnabled
    ? {
        draggable: true,
        onDragStart: (e: React.DragEvent) => {
          e.dataTransfer.effectAllowed = 'move';
          e.dataTransfer.setData('text/plain', leaf.containerId);
          reparentDrag.begin(leaf.containerId);
        },
        onDragEnd: () => {
          reparentDrag.end();
          setDropActive(false);
        },
        onDragOver: (e: React.DragEvent) => {
          if (!reparentDrag.canDropOn(leaf.containerId)) return;
          // preventDefault marks this row a valid drop target.
          e.preventDefault();
          e.dataTransfer.dropEffect = 'move';
          if (!dropActive) setDropActive(true);
        },
        onDragLeave: () => {
          if (dropActive) setDropActive(false);
        },
        onDrop: (e: React.DragEvent) => {
          e.preventDefault();
          setDropActive(false);
          if (reparentDrag.canDropOn(leaf.containerId)) {
            reparentDrag.drop(leaf.containerId);
          }
        },
      }
    : undefined;

  return (
    <>
      <TreeRow
        id={itemId(leaf.key)}
        level={3 + depth}
        expanded={hasChildren ? isOpen : undefined}
        ariaChecked={leaf.selected ? 'true' : 'false'}
        focused={focusedKey === leaf.key}
        onPointerSelect={() => onFocusKey(leaf.key)}
        onToggleSelect={() => onToggleLeaf(leaf)}
        onToggleExpand={
          hasChildren
            ? () => expansion.setExpanded(leaf.key, !isOpen)
            : undefined
        }
        dnd={dnd}
        dropActive={dropActive}
        className={
          'sidebar__row sidebar__row--leaf' +
          (isFocusTarget ? ' sidebar__row--focus-target' : '')
        }
        style={
          {
            ...(leaf.colorHex ? { '--container-color': leaf.colorHex } : {}),
            '--tree-depth': depth,
          } as React.CSSProperties
        }
      >
        {isEditing ? (
          <RenameField
            value={draft}
            onChange={onDraftChange}
            onCommit={onCommitEdit}
            onCancel={onCancelEdit}
            onKeyDown={onEditKey}
            ariaLabel={t('sidebar.renameInputLabel', { name: displayName })}
            hint={t('sidebar.renameHint')}
          />
        ) : (
          <>
            {hasChildren && (
              <span className="sidebar__chevron" aria-hidden="true">
                {isOpen ? '▾' : '▸'}
              </span>
            )}
            <span className="sidebar__swatch" aria-hidden="true" />
            <span className="sidebar__name">{displayName}</span>
          </>
        )}
      </TreeRow>
      {hasChildren &&
        isOpen &&
        leaf.children.map((child) => (
          <LeafRow
            key={child.key}
            leaf={child}
            depth={depth + 1}
            expansion={expansion}
            focusedKey={focusedKey}
            onFocusKey={onFocusKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={onDraftChange}
            onCancelEdit={onCancelEdit}
            onCommitEdit={onCommitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={onToggleLeaf}
            focusedContainerId={focusedContainerId}
            reparentDrag={reparentDrag}
            accountIsLocal={accountIsLocal}
          />
        ))}
    </>
  );
}

interface TreeRowProps {
  id: string;
  /** ARIA level: 1 account, 2 section, 3 leaf, 4+ nested task list. */
  level: number;
  /** Undefined ⇒ no twisty; row has no expand/collapse semantic. */
  expanded?: boolean;
  ariaChecked: 'true' | 'false' | 'mixed';
  focused: boolean;
  onPointerSelect: () => void;
  onToggleSelect: () => void;
  onToggleExpand?: () => void;
  ariaLabel?: string;
  className?: string;
  style?: React.CSSProperties;
  /** Reparent drag-and-drop handlers, spread onto the row. Present
   *  only on draggable rows (local task lists). */
  dnd?: Pick<
    React.HTMLAttributes<HTMLDivElement>,
    | 'draggable'
    | 'onDragStart'
    | 'onDragEnd'
    | 'onDragOver'
    | 'onDragLeave'
    | 'onDrop'
  >;
  /** True while a valid drag hovers this row — draws the drop ring. */
  dropActive?: boolean;
  children: React.ReactNode;
}

/**
 * One row in the tree. Renders an element with `role="treeitem"`,
 * the right `aria-level`, `aria-expanded` (where applicable), and
 * `aria-checked` for the tristate selection.
 *
 * Mouse interactions:
 *
 *   - clicking the chevron area (`.sidebar__chevron`) expands /
 *     collapses; clicking the rest toggles the checkbox.
 *
 * We don't put a `tabIndex` on the row — the parent tree owns the
 * single tab stop. Setting `data-focused="true"` lets the stylesheet
 * draw the visible focus ring on the right row.
 */
function TreeRow({
  id,
  level,
  expanded,
  ariaChecked,
  focused,
  onPointerSelect,
  onToggleSelect,
  onToggleExpand,
  ariaLabel,
  className,
  style,
  dnd,
  dropActive,
  children,
}: TreeRowProps) {
  return (
    <div
      id={id}
      role="treeitem"
      aria-level={level}
      aria-expanded={expanded}
      aria-checked={ariaChecked}
      aria-label={ariaLabel}
      data-focused={focused || undefined}
      className={
        (className ?? '') + (dropActive ? ' sidebar__row--drop-target' : '')
      }
      style={style}
      {...dnd}
      onClick={(e) => {
        // Buttons inside the row handle their own clicks (rename /
        // delete / chevron-area). The fall-through reaches a generic
        // toggle.
        const target = e.target as HTMLElement;
        if (target.closest('button, input')) return;
        onPointerSelect();
        if (
          onToggleExpand &&
          target.closest('.sidebar__chevron')
        ) {
          onToggleExpand();
          return;
        }
        onToggleSelect();
      }}
    >
      {children}
    </div>
  );
}

function triStateToAria(state: TriState): 'true' | 'false' | 'mixed' {
  switch (state) {
    case 'checked':
      return 'true';
    case 'mixed':
      return 'mixed';
    case 'unchecked':
      return 'false';
  }
}

// ────────────────────────────────────────────────────────────────────────
// Inline rename field
// ────────────────────────────────────────────────────────────────────────

function RenameField({
  value,
  onChange,
  onCommit,
  onCancel,
  onKeyDown,
  ariaLabel,
  hint,
}: {
  value: string;
  onChange: (v: string) => void;
  onCommit: (restoreFocus: boolean) => void;
  onCancel: (restoreFocus: boolean) => void;
  onKeyDown: (e: KeyboardEvent<HTMLInputElement>) => void;
  ariaLabel: string;
  hint: string;
}) {
  return (
    <div className="sidebar__rename">
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={() => onCommit(false)}
        aria-label={ariaLabel}
        autoFocus
        className="sidebar__rename-input"
      />
      <span className="sidebar__rename-hint" aria-hidden="true">
        {hint}
      </span>
      <button
        type="button"
        className="sidebar__rename-action"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onCommit(true)}
        aria-label={ariaLabel}
      >
        ✓
      </button>
      <button
        type="button"
        className="sidebar__rename-action"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onCancel(true)}
        aria-label={ariaLabel}
      >
        ✕
      </button>
    </div>
  );
}

