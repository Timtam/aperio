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
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveTaskColor } from '../../intl/eventColor';
import {
  priorityMarker,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskProgress,
  subtaskProgressSuffix,
} from '../../intl/taskStatus';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useDialogState } from '../../state/dialogStateContext';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import type { Section, Task } from '../../api/types';
import { duplicateTask } from '../duplicateActions';
import { buildEntries, DONE_GROUP_ID, type Entry } from './taskGrouping';
import { ConfirmDialog } from '../ConfirmDialog';
import { ColorPickerModal } from '../ColorPickerModal';
import {
  isCommandError,
  setSectionColor as setSectionColorCmd,
  showContextMenu,
  type ContextMenuItemRequest,
} from '../../api/client';

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
  // Section id → name, for the accessible task label (which section a
  // task sits in), flattened across all loaded lists.
  const sectionNameById = useMemo(() => {
    const m = new Map<string, string>();
    for (const list of Object.values(sectionsByList)) {
      for (const s of list) m.set(s.id, s.name);
    }
    return m;
  }, [sectionsByList]);

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
  const { openTaskDialog, openMoveCopy, openPlanTask, invalidateData } =
    useDialogState();

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
  const [collapsed, setCollapsed] = useState<Set<string>>(() =>
    loadDoneCollapsed() ? new Set([DONE_GROUP_ID]) : new Set(),
  );
  const toggleCollapsed = useCallback((id: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      // The Done group's open/closed choice persists across reloads;
      // per-subtree twisties stay session-local.
      if (id === DONE_GROUP_ID) saveDoneCollapsed(next.has(id));
      return next;
    });
  }, []);

  // Flatten the task buckets into a single options array, interleaved
  // with separator entries. focusIndex points at the *task* index in
  // `flatTasks` — separators never receive focus. Children appear
  // depth-first under their parent; the `hidden` flag on each entry
  // tells the renderer when the parent above is collapsed.
  const { entries, flatTasks } = useMemo(
    () => buildEntries(tasks, taskListById, t, collapsed, sectionsByList),
    [tasks, taskListById, t, collapsed, sectionsByList],
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

  const openSectionColorMenu = useCallback(
    async (section: Section, position?: { x: number; y: number }) => {
      if (colorLabels.length === 0) return;
      const items: ContextMenuItemRequest[] = [
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
      ];
      let selected: string | null = null;
      try {
        selected = await showContextMenu(items, position);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('show_context_menu failed', err);
        return;
      }
      if (selected === 'color:__other__') {
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
    [colorLabels, t, setSectionColor],
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

  // Shared toggle: WeekView and DayView use the same hook so the
  // Space-key contract is identical across every task surface.
  const toggleStatus = useTaskStatusToggle();
  const { openForTask: openTaskMenu } = useChipContextMenu();

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // The synthetic "Done (N)" group is a collapsible header, not a
      // task: Enter / Space toggle it; navigation + Arrow expand/collapse
      // fall through to the normal tree handling below; every task-only
      // shortcut (duplicate / move / plan / delete / context menu) is
      // inert so it never acts on a phantom task.
      if (flatTasks[focusIndex]?.id === DONE_GROUP_ID) {
        if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          toggleCollapsed(DONE_GROUP_ID);
          return;
        }
        const isNav =
          e.key.startsWith('Arrow') || e.key === 'Home' || e.key === 'End';
        if (!isNav) {
          if (e.key !== 'Tab') e.preventDefault();
          return;
        }
      }
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          const task = flatTasks[focusIndex];
          if (task) {
            void duplicateTask(task).then(() =>
              announce(t('actions.duplicated', { title: task.title })),
            );
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
          // Left on an expanded parent collapses it; on a child
          // jumps to the parent. Lets the user dismiss a noisy
          // sub-tree with one keystroke.
          e.preventDefault();
          const focused = focusedTaskEntry(entries, focusIndex);
          if (
            focused?.hasChildren &&
            !collapsed.has(focused.task.id)
          ) {
            toggleCollapsed(focused.task.id);
          } else if (focused && focused.task.parent_id) {
            const parentIdx = entries
              .filter(
                (e): e is Extract<Entry, { kind: 'task' }> =>
                  e.kind === 'task',
              )
              .find((e) => e.task.id === focused.task.parent_id)?.index;
            if (parentIdx !== undefined) setFocusIndex(parentIdx);
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
      itemId,
      entries,
      collapsed,
      toggleCollapsed,
    ],
  );

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
        {entries.map((entry, i) => {
          if (entry.kind === 'separator') {
            // level 1 = a section sub-header within a list; styled
            // smaller + indented to read as a child of the list head.
            const isSection = entry.level === 1;
            // A colored section tints its header (decorative — the
            // section name carries the meaning; the color also cascades
            // to the section's colorless task chips below).
            const sectionHex = entry.sectionId
              ? sectionColorById.get(entry.sectionId)
              : undefined;
            // The color action is offered for any section (a section's
            // color is a local concept — synced field for local lists, a
            // local override for external ones), so it doesn't depend on
            // the account. The header then becomes interactive (a menu
            // button + right-click target), so it can't stay aria-hidden.
            const section =
              entry.sectionId && entry.listId
                ? sectionsByList[entry.listId]?.find(
                    (s) => s.id === entry.sectionId,
                  )
                : undefined;
            const colorable = !!section && colorLabels.length > 0;
            return (
              <li
                key={`sep-${i}-${entry.label}`}
                role="presentation"
                aria-hidden={colorable ? undefined : true}
                className={
                  (isSection
                    ? 'task-list__group task-list__group--section'
                    : 'task-list__group') +
                  (sectionHex ? ' task-list__group--colored' : '')
                }
                style={
                  sectionHex
                    ? ({ '--event-color': sectionHex } as React.CSSProperties)
                    : undefined
                }
                onContextMenu={
                  colorable && section
                    ? (e) => {
                        e.preventDefault();
                        void openSectionColorMenu(section, {
                          x: e.clientX,
                          y: e.clientY,
                        });
                      }
                    : undefined
                }
              >
                {sectionHex && (
                  <span
                    className="task-list__group-swatch"
                    aria-hidden="true"
                  />
                )}
                {entry.label}
                {colorable && section && (
                  <button
                    type="button"
                    className="task-list__section-menu"
                    aria-label={t('views.tasks.sectionActions', {
                      name: section.name,
                    })}
                    onClick={(e) => {
                      const rect = e.currentTarget.getBoundingClientRect();
                      void openSectionColorMenu(section, {
                        x: rect.left,
                        y: rect.bottom,
                      });
                    }}
                  >
                    ⋮
                  </button>
                )}
              </li>
            );
          }
          // Children are rendered by their parent's recursive call
          // (the parent emits a <ul role="group"> below). The
          // top-level iteration only handles depth-0 tasks plus the
          // separator headings between them.
          if (entry.depth > 0) return null;
          return renderTreeItem(entry, {
            t,
            fmt,
            entries,
            tasks,
            taskListById,
            labelById,
            sectionColorById,
            sectionNameById,
            collapsed,
            toggleCollapsed,
            focusIndex,
            setFocusIndex,
            toggleStatus,
            openTaskDialog,
            openTaskMenu,
            itemId,
          });
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
  sectionNameById: Map<string, string>;
  collapsed: Set<string>;
  toggleCollapsed: (id: string) => void;
  focusIndex: number;
  setFocusIndex: (i: number) => void;
  toggleStatus: (task: Task) => Promise<void> | void;
  openTaskDialog: (task: Task) => void;
  openTaskMenu: (task: Task) => Promise<void> | void;
  itemId: (i: number) => string;
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
    sectionNameById,
    collapsed,
    toggleCollapsed,
    focusIndex,
    setFocusIndex,
    toggleStatus,
    openTaskDialog,
    openTaskMenu,
    itemId,
  } = ctx;
  const { task, listName, index, depth, hasChildren } = entry;
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
    if (e.kind === 'separator') break;
    if (e.depth <= depth) break;
    if (e.depth === depth + 1) children.push(e);
  }

  // The synthetic "Done (N)" group is a collapsible parent treeitem with
  // no status / due / checkbox — activating the row (click, or Enter /
  // Space / Arrow handled by the tree's keydown) only toggles expansion.
  // Modelling it as a treeitem (rather than a foreign <button>) keeps it
  // reachable by arrow keys and operable without hijacking the focused
  // task's Space/Enter action.
  if (task.id === DONE_GROUP_ID) {
    return (
      <li
        key={task.id}
        id={itemId(index)}
        role="treeitem"
        aria-selected={focused}
        aria-label={task.title}
        aria-level={depth + 1}
        aria-expanded={!isCollapsed}
        className={
          'task-list__item task-list__item--done-group' +
          (focused ? ' task-list__item--focused' : '')
        }
        onClick={(ev) => {
          // Only the header toggles — a click that lands on a completed
          // child row (which has its own onClick) must not also collapse
          // the group.
          if (
            (ev.target as HTMLElement).closest('.task-list__group-children')
          ) {
            return;
          }
          setFocusIndex(index);
          toggleCollapsed(task.id);
        }}
      >
        <span className="task-list__done-chevron" aria-hidden="true">
          {isCollapsed ? '▸' : '▾'}
        </span>
        <span className="task-list__done-label">{task.title}</span>
        {!isCollapsed && (
          <ul role="group" className="task-list__group-children">
            {children.map((child) => renderTreeItem(child, ctx))}
          </ul>
        )}
      </li>
    );
  }

  const due = describeDue(task, fmt, t);
  const color = resolveTaskColor(task, taskListById, labelById, sectionColorById);
  const marker = statusMarker(task.status);
  const priorityGlyph = priorityMarker(task.priority);
  const stateLabel = t(statusI18nKey(task.status));
  const progress = subtaskProgress(task.id, tasks);
  // The section a task sits in is part of its grouping, so name it in the
  // accessible label (like the list) — otherwise a screen-reader user
  // navigating by arrow keys can't tell which section a task belongs to
  // (the section header separators are decorative + skipped in nav).
  const sectionName = task.section_id
    ? sectionNameById.get(task.section_id)
    : undefined;
  const aria = t('views.tasks.optionLabel', {
    title: task.title,
    list: listName,
    state: stateLabel,
    priority: prioritySuffix(t, task.priority),
    progress: subtaskProgressSuffix(t, task.id, tasks),
    section: sectionName
      ? t('views.tasks.optionSectionSuffix', { name: sectionName })
      : '',
    due,
  });
  return (
    <li
      key={task.id}
      id={itemId(index)}
      role="treeitem"
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
        {priorityGlyph && (
          <span className="task-list__priority" aria-hidden="true">
            {priorityGlyph}{' '}
          </span>
        )}
        {task.title}
      </span>
      {task.assignees.length > 0 && (
        <span
          className="task-list__assignees"
          aria-label={t('views.tasks.assignedTo', {
            names: task.assignees.map((a) => a.name).join(', '),
          })}
        >
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

/** Find the task entry at flat-task position `index`, ignoring
 *  separators and respecting the index/entries decoupling. */
function focusedTaskEntry(
  entries: Entry[],
  index: number,
): Extract<Entry, { kind: 'task' }> | null {
  for (const e of entries) {
    if (e.kind === 'task' && e.index === index) return e;
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
): string {
  if (task.scheduled_date) {
    return t('views.tasks.dueScheduled', {
      date: fmt.format(new Date(task.scheduled_date), 'PP'),
    });
  }
  if (task.deadline_date) {
    return t('views.tasks.dueDeadline', {
      date: fmt.format(new Date(task.deadline_date), 'PP'),
    });
  }
  return t('views.tasks.dueNone');
}
