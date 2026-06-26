import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../../a11y/announcerContext';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useCurrentDayKey } from '../../hooks/useCurrentDayKey';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveTaskColor } from '../../intl/eventColor';
import {
  assigneeSuffix,
  priorityMarker,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskProgress,
  subtaskProgressSuffix,
} from '../../intl/taskStatus';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useCurrentUserByList } from '../../state/currentUser';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useDialogState } from '../../state/dialogStateContext';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import type { Section, Task } from '../../api/types';
import { duplicateTask } from '../duplicateActions';
import {
  buildEntries,
  DEFERRED_GROUP_ID,
  DONE_GROUP_ID,
  type Entry,
} from './taskGrouping';
import { suppressGroupHeaderKey } from './taskTreeKeys';
import { ConfirmDialog } from '../ConfirmDialog';
import { ColorPickerModal } from '../ColorPickerModal';
import {
  deleteSection,
  isCommandError,
  setSectionColor as setSectionColorCmd,
  showContextMenu,
  type ContextMenuItemRequest,
} from '../../api/client';
import {
  moveTaskToList,
  moveTaskToSection,
  readTaskDrag,
  setTaskDrag,
  TASK_DND_TYPE,
} from '../../state/moveActions';

/**
 * Dedicated task view — flat listbox of tasks with visual group
 * separators (Backlog + per-list).
 *
 * Why a listbox: a plain `tabIndex=-1` section is inert from NVDA's
 * point of view, which would let it fall back to browse mode and lose
 * arrow navigation. The listbox + `aria-activedescendant` pattern
 * mirrors the other Phase 3 views; the screen reader stays in focus
 * mode and reads the active option as it changes.
 *
 * Keyboard:
 *  - Arrow Up/Down move between tasks (separators are skipped).
 *  - Home/End jump to first/last task.
 *  - Space toggles the focused task's completion state.
 *
 * Filtering, sorting, and the wochenplan drag-and-drop arrive with
 * Phase 4.
 */
export function TaskView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { tasks, taskListById, loading } = useTasks();
  const { colorLabels, sectionsByList, loadSections, sectionColorById } =
    useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  // Pull sections for every list that currently has tasks. The fetch
  // is cached + cheap (section-less backends return [] via the trait
  // default), so we don't gate on the list's `sections` capability —
  // an empty result is the same "render flat" path.
  const listIdsWithTasks = useMemo(
    () => Array.from(new Set(tasks.map((task) => task.list_id))),
    [tasks],
  );
  useEffect(() => {
    for (const listId of listIdsWithTasks) {
      if (!(listId in sectionsByList)) void loadSections(listId);
    }
  }, [listIdsWithTasks, sectionsByList, loadSections]);
  const {
    openTaskDialog,
    openMoveCopy,
    openPlanTask,
    openSectionDialog,
    invalidateData,
  } = useDialogState();

  // Parent-task collapse state: the set of parent ids whose
  // children are currently hidden. Lives in component state so a
  // session-level collapse stays sticky as the user navigates the
  // list, but doesn't persist across reloads — a future polish
  // could move this into user_prefs the way the sidebar's
  // expansion map does.
  // Collapse set for subtree twisties + the synthetic "Done (N)" group
  // (keyed by DONE_GROUP_ID). The Done group's collapsed state is
  // persisted to localStorage and seeded here so finished tasks start
  // tucked away.
  const [collapsed, setCollapsed] = useState<Set<string>>(() => {
    const seed = new Set<string>();
    if (loadDoneCollapsed()) seed.add(DONE_GROUP_ID);
    if (loadDeferredCollapsed()) seed.add(DEFERRED_GROUP_ID);
    return seed;
  });
  const toggleCollapsed = useCallback((id: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      // The Done and Zukünftig groups persist their open/closed choice
      // across reloads; per-subtree twisties stay session-local.
      if (id === DONE_GROUP_ID) saveDoneCollapsed(next.has(id));
      else if (id === DEFERRED_GROUP_ID) saveDeferredCollapsed(next.has(id));
      return next;
    });
  }, []);

  // Local day key (refreshes at the date rollover) gates which backlog
  // tasks are still "future" and so live in the Zukünftig group.
  const todayKey = useCurrentDayKey();

  // Flatten the task buckets into a single options array, interleaved
  // with separator entries. focusIndex points at the *task* index in
  // `flatTasks` — separators never receive focus. Children appear
  // depth-first under their parent; the `hidden` flag on each entry
  // tells the renderer when the parent above is collapsed.
  const currentUserByList = useCurrentUserByList(tasks);
  const { entries, flatTasks } = useMemo(
    () =>
      buildEntries(
        tasks,
        taskListById,
        t,
        collapsed,
        sectionsByList,
        todayKey,
        currentUserByList,
      ),
    [
      tasks,
      taskListById,
      t,
      collapsed,
      sectionsByList,
      todayKey,
      currentUserByList,
    ],
  );

  const [focusIndex, setFocusIndex] = useState(0);

  useEffect(() => {
    if (focusIndex >= flatTasks.length) {
      setFocusIndex(Math.max(0, flatTasks.length - 1));
    }
  }, [flatTasks.length, focusIndex]);

  // Keep focus on a visible row: if the user collapses a parent
  // and the previously-focused subtask is now hidden, jump focus
  // back up to the parent so Arrow keys still land on something
  // useful. The visit walk emits parents before children, so the
  // closest non-hidden row above the current index is the parent.
  useEffect(() => {
    const focused = entries
      .filter((e): e is Extract<Entry, { kind: 'task' }> => e.kind === 'task')
      .find((e) => e.index === focusIndex);
    if (focused?.hidden) {
      for (let i = focusIndex - 1; i >= 0; i--) {
        const candidate = entries
          .filter(
            (e): e is Extract<Entry, { kind: 'task' }> => e.kind === 'task',
          )
          .find((e) => e.index === i);
        if (candidate && !candidate.hidden) {
          setFocusIndex(i);
          return;
        }
      }
    }
  }, [collapsed, entries, focusIndex]);

  // Deferred indicator — see DayView for the rationale.
  const showLoading = useDeferredLoading(loading);
  useEffect(() => {
    if (showLoading) announce(t('views.loading'));
  }, [showLoading, announce, t]);

  const idPrefix = useId();
  const itemId = useCallback(
    (i: number) => `${idPrefix}-item-${i}`,
    [idPrefix],
  );
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const [confirmTarget, setConfirmTarget] = useState<Task | null>(null);
  // Section whose custom ("other…") color is being composed in the
  // ColorPickerModal, opened from the section-header context menu.
  const [sectionColorTarget, setSectionColorTarget] = useState<Section | null>(
    null,
  );

  // Bind / clear / compose a section's color from its header context menu.
  // Mirrors the sidebar container color submenu. Persists via
  // `set_section_color`, which routes host-side: local sections store the
  // binding on their (synced) row; external sections store a local color
  // override. Relies on the live cascade to re-tint the section's
  // colorless tasks.
  const setSectionColor = useCallback(
    async (section: Section, labelId: string | null, colorName?: string) => {
      try {
        await setSectionColorCmd(section.id, section.list_id, labelId);
        await loadSections(section.list_id);
        announce(
          colorName
            ? t('views.tasks.sectionColorSet', {
                name: section.name,
                color: colorName,
              })
            : t('views.tasks.sectionColorCleared', { name: section.name }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set section color failed', err);
      }
    },
    [loadSections, announce, t],
  );

  // Full section-header menu (right-click + the ⋮ button): create /
  // rename / recolor / delete. Create + rename + delete show only where the
  // list's provider can manage sections (local, Todoist, Vikunja); the
  // colour submenu shows wherever colour labels exist (a section's colour is
  // a local concept even for external lists). Create/rename route to the
  // shared SectionDialog so a screen-reader user has a discoverable path
  // right where sections live — not only buried in the task editor.
  const openSectionMenu = useCallback(
    async (section: Section, position?: { x: number; y: number }) => {
      const manageable =
        taskListById.get(section.list_id)?.task_capabilities
          ?.manageable_sections ?? false;
      const colorable = colorLabels.length > 0;
      if (!manageable && !colorable) return;
      const items: ContextMenuItemRequest[] = [];
      if (manageable) {
        items.push({ id: 'add', label: t('dialogs.task.section.addAction') });
        items.push({ id: 'rename', label: t('dialogs.task.section.rename') });
      }
      if (colorable) {
        items.push({
          kind: 'submenu',
          label: t('sidebar.menu.color'),
          items: [
            {
              kind: 'check',
              id: 'color:__none__',
              label: t('sidebar.menu.colorNone'),
              checked: !section.color_label,
            },
            // Named palette labels only — hidden ad-hoc one-offs never appear.
            ...colorLabels
              .filter((cl) => !cl.ad_hoc)
              .map((cl) => ({
                kind: 'check' as const,
                id: `color:${cl.id}`,
                label: cl.name,
                checked: section.color_label === cl.id,
              })),
            { id: 'color:__other__', label: t('sidebar.menu.colorOther') },
          ],
        });
      }
      if (manageable) {
        items.push({ kind: 'separator' });
        items.push({ id: 'delete', label: t('dialogs.task.section.delete') });
      }
      let selected: string | null = null;
      try {
        selected = await showContextMenu(items, position);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('show_context_menu failed', err);
        return;
      }
      if (selected === 'add') {
        openSectionDialog(section.list_id, null);
      } else if (selected === 'rename') {
        openSectionDialog(section.list_id, section);
      } else if (selected === 'delete') {
        // Deleting a section only ungroups its tasks (ON DELETE SET NULL),
        // so there's no destructive confirm — the announce spells out that
        // the tasks survive.
        try {
          await deleteSection(section.id, section.list_id);
          await loadSections(section.list_id);
          announce(t('dialogs.task.section.deleted', { name: section.name }));
          invalidateData();
        } catch (err) {
          if (isCommandError(err)) {
            announce(`${err.code}: ${err.message}`);
          } else {
            announce(String(err));
          }
        }
      } else if (selected === 'color:__other__') {
        setSectionColorTarget(section);
      } else if (selected?.startsWith('color:')) {
        const raw = selected.slice('color:'.length);
        const labelId = raw === '__none__' ? null : raw;
        const labelName = labelId
          ? colorLabels.find((cl) => cl.id === labelId)?.name
          : undefined;
        await setSectionColor(section, labelId, labelName);
      }
    },
    [
      colorLabels,
      taskListById,
      t,
      setSectionColor,
      openSectionDialog,
      loadSections,
      announce,
      invalidateData,
    ],
  );

  const performDelete = useCallback(
    async (task: Task) => {
      try {
        await invoke<void>('delete_task', {
          id: task.id,
          listId: task.list_id,
        });
        announce(t('dialogs.task.deleted', { title: task.title }));
        // ConfirmDialog is local view-state, so the close here doesn't
        // run through DialogState — bump explicitly.
        invalidateData();
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t, invalidateData],
  );

  // Drag-and-drop: a task dropped on a section header moves into that
  // section. If the header belongs to a different list, the task moves to
  // that list first, then into the section. Mouse affordance only — the
  // keyboard/SR path stays the section field in the task dialog.
  const [dragOverSectionId, setDragOverSectionId] = useState<string | null>(
    null,
  );
  const dropTaskOnSection = useCallback(
    async (sectionId: string, sectionListId: string, e: React.DragEvent) => {
      e.preventDefault();
      setDragOverSectionId(null);
      const payload = readTaskDrag(e.dataTransfer);
      if (!payload) return;
      const { task, children } = payload;
      if (task.list_id === sectionListId && task.section_id === sectionId) {
        return; // already there
      }
      try {
        if (task.list_id === sectionListId) {
          await moveTaskToSection(task, sectionId);
        } else {
          const moved = await moveTaskToList(task, sectionListId, children);
          await moveTaskToSection(moved, sectionId);
        }
        invalidateData();
        announce(t('views.tasks.movedToSection', { title: task.title }));
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('drop task on section failed', err);
      }
    },
    [invalidateData, announce, t],
  );

  // Shared toggle: WeekView and DayView use the same hook so the
  // Space-key contract is identical across every task surface.
  const toggleStatus = useTaskStatusToggle();
  const { openForTask: openTaskMenu } = useChipContextMenu();

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Group headers (Backlog, a list, a section, the synthetic "Done (N)"
      // group) are collapsible tree rows, not tasks: Enter / Space toggle
      // them; a section header additionally opens its ⋮ menu via the menu
      // key; navigation + Arrow expand/collapse fall through to the normal
      // tree handling below; every task-only shortcut (duplicate / move /
      // plan / delete / task context menu) is inert so it never acts on a
      // phantom task.
      const focusedEntry = focusedTaskEntry(entries, focusIndex);
      if (focusedEntry?.group) {
        const grp = focusedEntry.group;
        if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          toggleCollapsed(focusedEntry.task.id);
          return;
        }
        if (
          grp.section &&
          (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey))
        ) {
          e.preventDefault();
          const node = (
            e.currentTarget as HTMLElement
          ).ownerDocument?.getElementById(itemId(focusIndex));
          const rect = node?.getBoundingClientRect();
          void openSectionMenu(
            grp.section,
            rect ? { x: rect.left, y: rect.bottom } : undefined,
          );
          return;
        }
        const isNav =
          e.key.startsWith('Arrow') || e.key === 'Home' || e.key === 'End';
        if (!isNav) {
          // Task-only shortcuts are inert on a group header — but OS / global
          // shortcuts (Alt+F4, Ctrl+R, …) and Tab must reach the window, so
          // only a plain key has its browser default suppressed.
          if (suppressGroupHeaderKey(e)) e.preventDefault();
          return;
        }
      }
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          const task = flatTasks[focusIndex];
          if (task) {
            void duplicateTask(task).then(() => {
              // The duplicate lands in task.list_id — the list currently in
              // view — so it must show without waiting for an unrelated bump.
              // (No dialog closes here to do it for us; mobile bumps too.)
              invalidateData();
              announce(t('actions.duplicated', { title: task.title }));
            });
          }
        }
        return;
      }
      if (e.altKey) return;
      if (e.shiftKey && e.key.toLowerCase() === 'm') {
        e.preventDefault();
        const task = flatTasks[focusIndex];
        if (task) openMoveCopy({ kind: 'task', task });
        return;
      }
      if (e.shiftKey && e.key.toLowerCase() === 'd') {
        // §9.3 — Shift+D opens the plan-task dialog so the user can
        // assign / change / clear the focused task's scheduled date.
        // Ctrl+D (above) is "duplicate" — different concern, intentionally
        // not collapsed.
        e.preventDefault();
        const task = flatTasks[focusIndex];
        if (task) openPlanTask(task);
        return;
      }
      if (flatTasks.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => nextVisibleIndex(entries, i, +1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          setFocusIndex((i) => nextVisibleIndex(entries, i, -1));
          return;
        case 'ArrowRight': {
          // Treeview convention: Right on a collapsed parent expands
          // it; on an expanded parent dives to the first child. We
          // implement only the expand half — diving is implicit
          // since the child sits one index below.
          e.preventDefault();
          const focused = focusedTaskEntry(entries, focusIndex);
          if (focused?.hasChildren && collapsed.has(focused.task.id)) {
            toggleCollapsed(focused.task.id);
          } else if (focused?.hasChildren) {
            setFocusIndex((i) => nextVisibleIndex(entries, i, +1));
          }
          return;
        }
        case 'ArrowLeft': {
          // Left on an expanded parent collapses it; on a leaf / collapsed
          // row jumps to the tree parent — a subtask's parent task, or the
          // group header (section / list / Backlog) the row sits under.
          // The parent is resolved by depth, not parent_id, so a top-level
          // task under a header (parent_id === null) still climbs to it.
          // Root-level rows (depth 0) have no parent and stay put.
          e.preventDefault();
          const focused = focusedTaskEntry(entries, focusIndex);
          if (focused?.hasChildren && !collapsed.has(focused.task.id)) {
            toggleCollapsed(focused.task.id);
          } else if (focused && focused.depth > 0) {
            const parent = parentEntry(entries, focused);
            if (parent) setFocusIndex(parent.index);
          }
          return;
        }
        case 'Home':
          e.preventDefault();
          setFocusIndex(firstVisibleIndex(entries));
          return;
        case 'End':
          e.preventDefault();
          setFocusIndex(lastVisibleIndex(entries));
          return;
        case ' ':
        case 'Spacebar': {
          e.preventDefault();
          const task = flatTasks[focusIndex];
          if (task) void toggleStatus(task);
          return;
        }
        case 'Enter': {
          e.preventDefault();
          const task = flatTasks[focusIndex];
          if (task) openTaskDialog(task);
          return;
        }
        case 'Delete':
        case 'Backspace': {
          e.preventDefault();
          const task = flatTasks[focusIndex];
          if (task) setConfirmTarget(task);
          return;
        }
        case 'ContextMenu':
        case 'F10': {
          if (e.key === 'F10' && !e.shiftKey) return;
          e.preventDefault();
          const task = flatTasks[focusIndex];
          if (task) {
            const target = e.currentTarget as HTMLElement;
            const id = itemId(focusIndex);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            const pos = rect
              ? { x: rect.left, y: rect.bottom }
              : undefined;
            void openTaskMenu(task, pos);
          }
          return;
        }
        default:
          return;
      }
    },
    [
      flatTasks,
      focusIndex,
      toggleStatus,
      openTaskDialog,
      openMoveCopy,
      openPlanTask,
      announce,
      t,
      openTaskMenu,
      openSectionMenu,
      itemId,
      entries,
      collapsed,
      toggleCollapsed,
      invalidateData,
    ],
  );

  // Shared context for the recursive task-row renderer.
  const renderCtx = {
    t,
    fmt,
    entries,
    tasks,
    taskListById,
    labelById,
    sectionColorById,
    collapsed,
    toggleCollapsed,
    focusIndex,
    setFocusIndex,
    toggleStatus,
    openTaskDialog,
    openTaskMenu,
    itemId,
    today: todayKey,
  };

  // Dispatch a row. A real task delegates to renderTreeItem (which renders
  // its own subtask subtree); a group header (Backlog / list / section /
  // Done) renders here as a collapsible treeitem — defined in the component
  // so it keeps the colour + ⋮ + drop-target closures the old visual header
  // had, while now being a real, arrow-reachable tree node.
  function renderNode(entry: Entry): React.ReactNode {
    if (!entry.group) {
      return renderTreeItem(entry, renderCtx);
    }
    const idx = entries.indexOf(entry);
    const children: Entry[] = [];
    for (let i = idx + 1; i < entries.length; i++) {
      const e = entries[i];
      if (e.depth <= entry.depth) break;
      if (e.depth === entry.depth + 1) children.push(e);
    }
    return renderGroupHeader(entry, children);
  }

  function renderGroupHeader(entry: Entry, children: Entry[]): React.ReactNode {
    const meta = entry.group!;
    const id = entry.task.id;
    const focused = entry.index === focusIndex;
    const isCollapsed = collapsed.has(id);
    const section = meta.section;
    const sectionHex = meta.sectionId
      ? sectionColorById.get(meta.sectionId)
      : undefined;
    const sectionManageable = section
      ? (taskListById.get(section.list_id)?.task_capabilities
          ?.manageable_sections ?? false)
      : false;
    const sectionActionable =
      !!section && (sectionManageable || colorLabels.length > 0);
    return (
      <li
        key={id}
        id={itemId(entry.index)}
        role="treeitem"
        aria-selected={focused}
        aria-label={entry.task.title}
        aria-level={entry.depth + 1}
        aria-expanded={entry.hasChildren ? !isCollapsed : undefined}
        className={
          'task-list__item task-list__group-row' +
          ` task-list__group-row--${meta.kind}` +
          (focused ? ' task-list__item--focused' : '') +
          (sectionHex ? ' task-list__group-row--colored' : '') +
          (section && dragOverSectionId === section.id
            ? ' task-list__group-row--drop-active'
            : '')
        }
        style={
          {
            '--task-depth': entry.depth,
            ...(sectionHex ? { '--event-color': sectionHex } : {}),
          } as React.CSSProperties
        }
        onClick={(ev) => {
          // The header toggles its own group; a click on one of THIS header's
          // own child rows must NOT also collapse it (the child has its own
          // handler). We test this header's OWN children container directly
          // (`:scope >`) rather than `target.closest('.group-children')`,
          // because a NESTED header (a list under Backlog) lives inside its
          // PARENT's `.group-children` — the broad `closest` matched that and
          // swallowed the nested header's own twisty click. The ⋮ button stops
          // propagation itself, so it never reaches here.
          const ownChildren = ev.currentTarget.querySelector(
            ':scope > .task-list__group-children',
          );
          if (ownChildren?.contains(ev.target as Node)) {
            return;
          }
          setFocusIndex(entry.index);
          toggleCollapsed(id);
        }}
        onContextMenu={
          sectionActionable && section
            ? (e) => {
                e.preventDefault();
                e.stopPropagation();
                setFocusIndex(entry.index);
                void openSectionMenu(section, { x: e.clientX, y: e.clientY });
              }
            : undefined
        }
        onDragOver={
          section
            ? (e) => {
                if (!e.dataTransfer.types.includes(TASK_DND_TYPE)) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = 'move';
                if (dragOverSectionId !== section.id) {
                  setDragOverSectionId(section.id);
                }
              }
            : undefined
        }
        onDragLeave={
          section
            ? () =>
                setDragOverSectionId((cur) =>
                  cur === section.id ? null : cur,
                )
            : undefined
        }
        onDrop={
          section
            ? (e) => void dropTaskOnSection(section.id, section.list_id, e)
            : undefined
        }
      >
        <span className="task-list__group-twisty" aria-hidden="true">
          {entry.hasChildren ? (isCollapsed ? '▸' : '▾') : ''}
        </span>
        {sectionHex && (
          <span className="task-list__group-swatch" aria-hidden="true" />
        )}
        <span className="task-list__group-label">{entry.task.title}</span>
        {sectionActionable && section && (
          <button
            type="button"
            className="task-list__section-menu"
            aria-label={t('views.tasks.sectionActions', {
              name: section.name,
            })}
            onClick={(e) => {
              e.stopPropagation();
              const rect = e.currentTarget.getBoundingClientRect();
              void openSectionMenu(section, { x: rect.left, y: rect.bottom });
            }}
            onKeyDown={(e) => {
              if (
                e.key === 'ContextMenu' ||
                (e.key === 'F10' && e.shiftKey)
              ) {
                e.preventDefault();
                e.stopPropagation();
                const rect = e.currentTarget.getBoundingClientRect();
                void openSectionMenu(section, {
                  x: rect.left,
                  y: rect.bottom,
                });
              }
            }}
          >
            ⋮
          </button>
        )}
        {entry.hasChildren && !isCollapsed && (
          <ul role="group" className="task-list__group-children">
            {children.map((child) => renderNode(child))}
          </ul>
        )}
      </li>
    );
  }

  return (
    <section className="view view--tasks" aria-label={t('views.tasks.title')}>
      <header className="view__header">
        <h2>{t('views.tasks.title')}</h2>
      </header>

      {showLoading && (
        <p className="view__loading" aria-hidden="true">
          {t('views.loading')}
        </p>
      )}

      <ul
        ref={listRef}
        role="tree"
        tabIndex={0}
        aria-label={t('views.tasks.taskList')}
        aria-activedescendant={
          flatTasks.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="task-list"
      >
        {/* Real W3C tree — role=tree on the container, role=treeitem
            on each row, and a nested role=group for the children of
            every expanded parent. The flat `entries` array (DFS-
            ordered) still drives keyboard nav and focus indexing via
            `flatTasks`; the recursive render builds the matching DOM
            hierarchy so AT actually announces "level 2, expanded,
            3 children" instead of "option 5 of 12" with invisible
            aria-level attributes. Parent toggle is intentionally
            isolated — Space on a parent flips its own status only,
            children keep theirs. */}
        {flatTasks.length === 0 && (
          <li role="presentation" className="task-list__empty">
            {t('views.tasks.empty')}
          </li>
        )}
        {entries.map((entry) => {
          if (entry.depth > 0) return null;
          return renderNode(entry);
        })}
      </ul>

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget);
        }}
        title={t('dialogs.confirm.deleteTaskTitle')}
        message={t('dialogs.confirm.deleteTaskMessage', {
          title: confirmTarget?.title ?? '',
        })}
      />
      <ColorPickerModal
        isOpen={sectionColorTarget !== null}
        onClose={() => setSectionColorTarget(null)}
        initialHex={
          sectionColorTarget
            ? sectionColorById.get(sectionColorTarget.id)
            : undefined
        }
        onResolve={(label) => {
          if (sectionColorTarget) {
            void setSectionColor(sectionColorTarget, label.id, label.name);
          }
        }}
      />
    </section>
  );
}

/**
 * Render context for the recursive `renderTreeItem` walker. All
 * the data + callbacks the tree needs travel as a single object so
 * we don't thread two dozen positional arguments through the
 * recursion. The shape stays internal to this module — exporting
 * it would be a noise tax on the rest of the app.
 */
interface RenderTreeCtx {
  t: (key: string, vars?: Record<string, unknown>) => string;
  fmt: ReturnType<typeof useDateFormat>;
  entries: Entry[];
  tasks: Task[];
  taskListById: Map<string, import('../../api/types').TaskList>;
  labelById: Map<string, import('../../api/types').ColorLabel>;
  sectionColorById: Map<string, string>;
  collapsed: Set<string>;
  toggleCollapsed: (id: string) => void;
  focusIndex: number;
  setFocusIndex: (i: number) => void;
  toggleStatus: (task: Task) => Promise<void> | void;
  openTaskDialog: (task: Task) => void;
  openTaskMenu: (task: Task) => Promise<void> | void;
  itemId: (i: number) => string;
  /** Local `YYYY-MM-DD` — lets a row show its future resurface date. */
  today: string;
}

/**
 * Render one node in the task tree, recursing into a nested
 * `<ul role="group">` when the node is an expanded parent. The DOM
 * mirrors the ARIA pattern: each `<li role="treeitem">` owns its
 * own group, which AT announces as "expanded, N items".
 */
function renderTreeItem(
  entry: Extract<Entry, { kind: 'task' }>,
  ctx: RenderTreeCtx,
): JSX.Element {
  const {
    t,
    fmt,
    entries,
    tasks,
    taskListById,
    labelById,
    sectionColorById,
    collapsed,
    toggleCollapsed,
    focusIndex,
    setFocusIndex,
    toggleStatus,
    openTaskDialog,
    openTaskMenu,
    itemId,
    today,
  } = ctx;
  const { task, index, depth, hasChildren } = entry;
  const focused = index === focusIndex;
  const isCollapsed = collapsed.has(task.id);
  // Direct children (depth+1, same parent_id) — discovered via the
  // entries array so we don't have to maintain a separate children
  // map. `entries` is DFS-ordered, so the immediate children sit
  // between this entry and the next entry of equal-or-shallower
  // depth.
  const children: Extract<Entry, { kind: 'task' }>[] = [];
  const myIdx = entries.indexOf(entry);
  for (let i = myIdx + 1; i < entries.length; i++) {
    const e = entries[i];
    if (e.depth <= depth) break;
    if (e.depth === depth + 1) children.push(e);
  }

  const due = describeDue(task, fmt, t, today);
  const color = resolveTaskColor(task, taskListById, labelById, sectionColorById);
  const marker = statusMarker(task.status);
  const priorityGlyph = priorityMarker(task.priority);
  const progress = subtaskProgress(task.id, tasks);
  // The list and section a task sits in are now conveyed structurally by
  // the surrounding group-header treeitems (a screen-reader user lands on
  // "Backlog → Inbox → To Do" before reaching the task), so the row's own
  // label no longer repeats them.
  const aria = t('views.tasks.optionLabel', {
    title: task.title,
    state: t(statusI18nKey(task.status)),
    priority: prioritySuffix(t, task.priority),
    progress: subtaskProgressSuffix(t, task.id, tasks),
    due,
    assignee: assigneeSuffix(t, task.assignees),
  });
  return (
    <li
      key={task.id}
      id={itemId(index)}
      role="treeitem"
      draggable
      onDragStart={(e) => {
        const childRows = tasks.filter((c) => c.parent_id === task.id);
        setTaskDrag(e.dataTransfer, task, childRows);
      }}
      aria-selected={focused}
      aria-label={aria}
      aria-level={depth + 1}
      aria-expanded={hasChildren ? !isCollapsed : undefined}
      className={
        'task-list__item' +
        (focused ? ' task-list__item--focused' : '') +
        ` task-list__item--${task.status.replace('_', '-')}` +
        (depth > 0 ? ' task-list__item--child' : '')
      }
      style={
        {
          ...(color.hex ? { '--event-color': color.hex } : {}),
          // Indentation per depth — driven by a custom prop so the
          // rest of the grid columns (check / title / progress / due)
          // keep their alignment.
          '--task-depth': depth,
        } as React.CSSProperties
      }
      onClick={() => {
        // Clicking the row opens the editor — consistent with the task
        // chips in the calendar views. Checking off is the marker's job
        // (below), so a stray click doesn't flip a task's status.
        setFocusIndex(index);
        openTaskDialog(task);
      }}
      onContextMenu={(ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        setFocusIndex(index);
        void openTaskMenu(task);
      }}
    >
      {hasChildren ? (
        <button
          type="button"
          className="task-list__twisty"
          aria-label={t(
            isCollapsed ? 'views.tasks.expand' : 'views.tasks.collapse',
            { title: task.title },
          )}
          // Clicking the twisty is its own action — stop propagation
          // so the row's onClick (toggle status) doesn't also fire.
          onClick={(ev) => {
            ev.stopPropagation();
            toggleCollapsed(task.id);
          }}
        >
          <span aria-hidden="true">{isCollapsed ? '▸' : '▾'}</span>
        </button>
      ) : (
        <span aria-hidden="true" className="task-list__twisty-spacer" />
      )}
      <span
        className="task-list__check task-list__check--clickable"
        aria-hidden="true"
        // Mouse: clicking the marker toggles the task's done state
        // without opening the editor (the row's onClick). Keyboard/SR
        // users toggle with Space on the focused row (unchanged).
        onClick={(ev) => {
          ev.stopPropagation();
          setFocusIndex(index);
          void toggleStatus(task);
        }}
      >
        {marker}
      </span>
      <span className="task-list__title">
        {task.title}
        {priorityGlyph && (
          <span className="task-list__priority" aria-hidden="true">
            {' '}
            {priorityGlyph}
          </span>
        )}
      </span>
      {task.assignees.length > 0 && (
        // Decorative: the assignee is announced via the row's aria-label
        // (the assignee suffix), so the visible chip is hidden from AT to
        // avoid a double read — same pattern as the priority / progress
        // spans.
        <span className="task-list__assignees" aria-hidden="true">
          {task.assignees.map((a) => a.name).join(', ')}
        </span>
      )}
      {progress && (
        <span
          className="task-list__progress"
          aria-hidden="true"
          title={t('views.tasks.subtaskProgressBadgeAria', progress)}
        >
          {t('views.tasks.subtaskProgressBadge', progress)}
        </span>
      )}
      <span className="task-list__due">{due}</span>
      {hasChildren && !isCollapsed && (
        // Nested ARIA group. AT announces this as "group, N items".
        // Hidden completely (not just CSS-hidden) when collapsed so
        // the tree doesn't claim items the user can't reach. The
        // flat focus index still tracks children — clamp effect
        // pulls focus up if the user collapses while a child is
        // focused.
        <ul role="group" className="task-list__group-children">
          {children.map((child) => renderTreeItem(child, ctx))}
        </ul>
      )}
    </li>
  );
}

const DONE_COLLAPSED_KEY = 'aperio.tasks.doneCollapsed';

/** Read the persisted "Done group collapsed" preference. Defaults to
 *  collapsed (true) — the whole point is to keep finished tasks out of
 *  the way — and tolerates a missing / unreadable store. */
function loadDoneCollapsed(): boolean {
  try {
    return localStorage.getItem(DONE_COLLAPSED_KEY) !== 'false';
  } catch {
    return true;
  }
}

function saveDoneCollapsed(value: boolean): void {
  try {
    localStorage.setItem(DONE_COLLAPSED_KEY, String(value));
  } catch {
    // Private-mode / quota errors are non-fatal — the toggle still
    // works for the session, it just won't persist.
  }
}

const DEFERRED_COLLAPSED_KEY = 'aperio.tasks.deferredCollapsed';

/** Read the persisted "Zukünftig group collapsed" preference. Defaults to
 *  collapsed (true) — future tasks shouldn't crowd today's work — and
 *  tolerates a missing / unreadable store. */
function loadDeferredCollapsed(): boolean {
  try {
    return localStorage.getItem(DEFERRED_COLLAPSED_KEY) !== 'false';
  } catch {
    return true;
  }
}

function saveDeferredCollapsed(value: boolean): void {
  try {
    localStorage.setItem(DEFERRED_COLLAPSED_KEY, String(value));
  } catch {
    // Non-fatal — see saveDoneCollapsed.
  }
}

/** Find the row at flat-task position `index` (respecting the
 *  index/entries decoupling). Every row — real task or group header — is a
 *  navigable entry, so this resolves both. */
function focusedTaskEntry(entries: Entry[], index: number): Entry | null {
  for (const e of entries) {
    if (e.index === index) return e;
  }
  return null;
}

/** The tree parent of `entry`: the nearest preceding row (DFS order) one
 *  depth shallower. Unlike `parent_id` — which links a subtask to its parent
 *  *task* only — this resolves the structural parent for every row: a task
 *  under a section / list / Backlog header, or a header under another header.
 *  Returns null at the root level (depth 0 has no parent). `entry.index` is
 *  its own position in `entries` (entries + flatTasks grow in lockstep), so
 *  we scan back from it without an identity-based lookup. */
function parentEntry(entries: Entry[], entry: Entry): Entry | null {
  for (let i = entry.index - 1; i >= 0; i--) {
    if (entries[i].depth === entry.depth - 1) return entries[i];
  }
  return null;
}

/** Next index in the requested direction that points at a visible
 *  (non-hidden) row. Clamps at the boundary so the user never
 *  wraps past the end of the list. */
function nextVisibleIndex(
  entries: Entry[],
  current: number,
  dir: 1 | -1,
): number {
  const tasks = entries.filter(
    (e): e is Extract<Entry, { kind: 'task' }> => e.kind === 'task',
  );
  let cursor = current + dir;
  while (cursor >= 0 && cursor < tasks.length) {
    const candidate = tasks.find((e) => e.index === cursor);
    if (candidate && !candidate.hidden) return cursor;
    cursor += dir;
  }
  return current;
}

function firstVisibleIndex(entries: Entry[]): number {
  for (const e of entries) {
    if (e.kind === 'task' && !e.hidden) return e.index;
  }
  return 0;
}

function lastVisibleIndex(entries: Entry[]): number {
  let found = 0;
  for (const e of entries) {
    if (e.kind === 'task' && !e.hidden) found = e.index;
  }
  return found;
}

function describeDue(
  task: Task,
  fmt: ReturnType<typeof useDateFormat>,
  t: (key: string, vars?: Record<string, unknown>) => string,
  today: string,
): string {
  // A finished task shows WHEN it was finished — the scheduled/deadline date
  // is moot once it's done. Flows into both the visible due column and the
  // row's aria-label (which is built from `due`).
  if (task.status === 'completed' && task.completed_at) {
    return t('views.tasks.completedAt', {
      date: fmt.format(new Date(task.completed_at), 'PPP'),
    });
  }
  // A deferred backlog task (DESIGN §9.12) shows WHEN it will resurface —
  // the only date that matters while it waits in the Zukünftig group.
  if (task.resurface_date && task.resurface_date > today) {
    return t('views.tasks.resurfacesOn', {
      date: fmt.format(new Date(task.resurface_date), 'PPP'),
    });
  }
  if (task.scheduled_date) {
    return t('views.tasks.dueScheduled', {
      date: fmt.format(new Date(task.scheduled_date), 'PPP'),
    });
  }
  if (task.deadline_date) {
    return t('views.tasks.dueDeadline', {
      date: fmt.format(new Date(task.deadline_date), 'PPP'),
    });
  }
  return t('views.tasks.dueNone');
}
