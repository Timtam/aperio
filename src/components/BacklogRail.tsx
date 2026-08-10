import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import { backlogWeeks, splitDeadlinesByWeek } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { isCommandError } from '../api/client';
import type { Task } from '../api/types';
import { duplicateTask } from './duplicateActions';
import { useDateFormat } from '../intl/dateFormat';
import { labelsLookup, resolveTaskColor } from '../intl/eventColor';
import {
  assigneeSuffix,
  effortSizeModifier,
  effortSuffix,
  priorityMarker,
  priorityRank,
  prioritySuffix,
  subtaskParentTitle,
} from '../intl/taskStatus';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useTaskCascadeEnabled } from '../state/taskCascadeContext';
import { useDialogState } from '../state/dialogStateContext';
import {
  moveTaskToBacklog,
  readTaskDrag,
  setTaskDrag,
  TASK_DND_TYPE,
} from '../state/moveActions';
import {
  BACKLOG_WIDTH_MAX,
  BACKLOG_WIDTH_MIN,
  useBacklogWidth,
} from '../state/useBacklogWidth';
import { useChipContextMenu } from '../state/useChipContextMenu';
import { useViewState } from '../state/viewStateContext';
import { useTasks } from '../state/useTasks';
import { useCurrentDayKey } from '../hooks/useCurrentDayKey';
import { isTaskDeferred } from './views/taskGrouping';

/** Parse a `YYYY-MM-DD` day key into a LOCAL Date (no UTC drift). */
function parseDayKey(key: string): Date {
  const [y, m, d] = key.split('-').map(Number);
  return new Date(y, m - 1, d);
}

/**
 * Backlog rail for the week / month planner — three sections, by horizon.
 *
 *   - **This week** (top): deadlines falling in the CALENDAR week that holds
 *     today, which is not "the next seven days" — it ends when the week ends,
 *     wherever the user's week-start setting puts that. Overdue deadlines land
 *     here too: the date sort puts them at the very top of the whole rail, and
 *     the tail is the last place the most urgent thing belongs.
 *   - **Next week**: the calendar week after that one, same rule.
 *   - **Everything else**: the deadlines beyond next week, still by date, and
 *     below them the classic priority backlog — open / in-progress, top-level
 *     tasks with no `scheduled_date` AND no deadline, not deferred, high → low.
 *
 * A deadline task appears regardless of the day plan, so one already scheduled
 * onto a day still shows here (it also keeps its grid due-marker) and a started
 * task is included — the rail is the one place to see what's due soonest.
 *
 * All three lists are drag sources (drop a chip on a day cell to schedule it)
 * and the rail is a drop target (drop a scheduled task back here to clear its
 * plan). Each list is its own single-tab-stop `listbox` (Arrow/Home/End move the
 * active option via `aria-activedescendant`, Enter opens, Shift+D plans,
 * ContextMenu / Shift+F10 opens the task menu), so the rail stays fully
 * keyboard/SR usable.
 *
 * Rendered as a fixed-width resizable column left of the grid; it joins the F6
 * region cycle. Each chip carries its hierarchical color (task → section → list)
 * and a priority marker; deadline chips additionally show the due date.
 */
export function BacklogRail() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const todayKey = useCurrentDayKey();
  const { tasks } = useTasks();
  const { invalidateData } = useDialogState();
  const { colorLabels, sectionsByList, loadSections } = useCalendarStore();
  const headingId = useId();
  const { width, setWidth } = useBacklogWidth();
  // Where a week begins is the user's setting, not a constant — the same one
  // the month grid lays its columns out by.
  const { weekStartsOn } = useViewState();
  const rootRef = useRef<HTMLElement>(null);

  // Drag the column's right edge to resize; the width persists and the grid
  // beside it flexes to fill the rest.
  const beginResize = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      const handle = e.currentTarget;
      const left = rootRef.current?.getBoundingClientRect().left ?? 0;
      handle.setPointerCapture(e.pointerId);
      const onMove = (ev: PointerEvent) => setWidth(ev.clientX - left);
      const onUp = () => {
        handle.releasePointerCapture(e.pointerId);
        handle.removeEventListener('pointermove', onMove);
        handle.removeEventListener('pointerup', onUp);
      };
      handle.addEventListener('pointermove', onMove);
      handle.addEventListener('pointerup', onUp);
    },
    [setWidth],
  );

  // Keyboard resize for the separator: Left/Right by a step, Home/End to the
  // bounds — keeps the splitter operable without a pointer.
  const onResizeKey = useCallback(
    (e: React.KeyboardEvent) => {
      const STEP = 16;
      if (e.key === 'ArrowLeft') {
        e.preventDefault();
        setWidth(width - STEP);
      } else if (e.key === 'ArrowRight') {
        e.preventDefault();
        setWidth(width + STEP);
      } else if (e.key === 'Home') {
        e.preventDefault();
        setWidth(BACKLOG_WIDTH_MIN);
      } else if (e.key === 'End') {
        e.preventDefault();
        setWidth(BACKLOG_WIDTH_MAX);
      }
    },
    [setWidth, width],
  );

  const ids = useMemo(() => new Set(tasks.map((row) => row.id)), [tasks]);
  // top-level (or orphaned) — subtasks travel with their parent.
  const isTopLevel = useCallback(
    (row: Task) => !row.parent_id || !ids.has(row.parent_id),
    [ids],
  );

  // Deadline section: ALL open / in-progress tasks with a deadline (scheduled or
  // not, started or not), earliest deadline first; priority then created-order
  // break ties within one date.
  const deadlineTasks = useMemo(
    () =>
      tasks
        .filter(
          // Include subtasks WITH their own deadline (each chip is labelled by
          // its parent); an undated subtask still travels with its parent and
          // stays out of the rail.
          (row) => row.status !== 'completed' && !!row.deadline_date,
        )
        .sort(
          (a, b) =>
            (a.deadline_date ?? '').localeCompare(b.deadline_date ?? '') ||
            priorityRank(a.priority) - priorityRank(b.priority) ||
            a.created_at.localeCompare(b.created_at),
        ),
    [tasks],
  );

  // The two calendar weeks the deadlines are split across. The user's own
  // week-start setting decides where a week begins — see `backlogWeeks`.
  const weeks = useMemo(
    () => backlogWeeks(todayKey, weekStartsOn),
    [todayKey, weekStartsOn],
  );
  const byWeek = useMemo(
    () => splitDeadlinesByWeek(deadlineTasks, weeks),
    [deadlineTasks, weeks],
  );

  // Priority backlog: the classic active backlog MINUS deadline tasks (those are
  // in the sections above), high → low priority.
  const priorityBacklog = useMemo(
    () =>
      tasks
        .filter(
          (row) =>
            row.status !== 'completed' &&
            !row.scheduled_date &&
            // deadline tasks are placed by their horizon — keep them out of
            // here so a task never appears in two sections.
            !row.deadline_date &&
            // a deferred backlog task (resurfaces on a future day) waits in the
            // task view's "Zukünftig" group, not the active rail.
            !isTaskDeferred(row, todayKey) &&
            isTopLevel(row),
        )
        .sort((a, b) => priorityRank(a.priority) - priorityRank(b.priority)),
    [tasks, todayKey, isTopLevel],
  );

  // Everything else, as one list: the deadlines beyond next week first — still
  // by date, so the nearest is nearest the top — and the undated backlog below
  // them by priority. Two populations, one section, because that is what "the
  // rest" means to someone reading down the column.
  const everythingElse = useMemo(
    () => [...byWeek.later, ...priorityBacklog],
    [byWeek.later, priorityBacklog],
  );

  // Per-chip color follows the task → section → list chain. Sections load
  // lazily, so trigger a load for every list that has a rail task.
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);
  const listIds = useMemo(
    () =>
      Array.from(
        new Set(
          [...deadlineTasks, ...priorityBacklog].map((task) => task.list_id),
        ),
      ),
    [deadlineTasks, priorityBacklog],
  );
  useEffect(() => {
    for (const listId of listIds) {
      if (!(listId in sectionsByList)) void loadSections(listId);
    }
  }, [listIds, sectionsByList, loadSections]);

  const dropToBacklog = async (e: React.DragEvent) => {
    e.preventDefault();
    const payload = readTaskDrag(e.dataTransfer);
    if (!payload) return;
    const { task } = payload;
    if (!task.scheduled_date && !task.deadline_date) return; // already backlog
    try {
      await moveTaskToBacklog(task);
      invalidateData();
      announce(t('views.backlog.movedToBacklog', { title: task.title }));
    } catch (err) {
      announce(
        isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
      );
    }
  };

  const total = deadlineTasks.length + priorityBacklog.length;

  return (
    <section
      ref={rootRef}
      className="backlog-rail"
      data-region="backlog"
      aria-labelledby={headingId}
      style={{ flexBasis: `${width}px` }}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(TASK_DND_TYPE)) return;
        // A task dragged over the rail → valid "back to backlog" drop.
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
      }}
      onDrop={(e) => void dropToBacklog(e)}
    >
      <h2 id={headingId} className="backlog-rail__heading">
        {t('views.backlog.heading', { count: total })}
      </h2>
      <div className="backlog-rail__body">
        {total === 0 ? (
          <p className="backlog-rail__empty">{t('views.backlog.empty')}</p>
        ) : (
          <>
            {byWeek.thisWeek.length > 0 && (
              <BacklogList
                items={byWeek.thisWeek}
                heading={t('views.backlog.thisWeekHeading', {
                  count: byWeek.thisWeek.length,
                })}
                listLabel={t('views.backlog.thisWeekListLabel')}
                optionPrefix="backlog-this-week"
                labelById={labelById}
                todayKey={todayKey}
                showDeadline
              />
            )}
            {byWeek.nextWeek.length > 0 && (
              <BacklogList
                items={byWeek.nextWeek}
                heading={t('views.backlog.nextWeekHeading', {
                  count: byWeek.nextWeek.length,
                })}
                listLabel={t('views.backlog.nextWeekListLabel')}
                optionPrefix="backlog-next-week"
                labelById={labelById}
                todayKey={todayKey}
                showDeadline
              />
            )}
            {everythingElse.length > 0 && (
              <BacklogList
                items={everythingElse}
                heading={t('views.backlog.restHeading', {
                  count: everythingElse.length,
                })}
                listLabel={t('views.backlog.restListLabel')}
                optionPrefix="backlog-rest"
                labelById={labelById}
                todayKey={todayKey}
                // The section mixes dated and undated rows; a chip without a
                // deadline simply renders none.
                showDeadline
              />
            )}
          </>
        )}
      </div>
      {/* Drag (or arrow-key) the right edge to resize the column. */}
      <div
        className="backlog-rail__resizer"
        role="separator"
        aria-orientation="vertical"
        aria-label={t('views.backlog.resize')}
        aria-valuenow={width}
        aria-valuemin={BACKLOG_WIDTH_MIN}
        aria-valuemax={BACKLOG_WIDTH_MAX}
        tabIndex={0}
        onPointerDown={beginResize}
        onKeyDown={onResizeKey}
      />
    </section>
  );
}

interface BacklogListProps {
  items: Task[];
  heading: string;
  listLabel: string;
  /** Unique id prefix so each list's `aria-activedescendant` targets are
   *  distinct across the listboxes in the rail. */
  optionPrefix: string;
  labelById: ReturnType<typeof labelsLookup>;
  todayKey: string;
  /** Render the due date on each chip; a chip without a deadline shows none. */
  showDeadline?: boolean;
}

/** One labelled `listbox` section of the rail. */
function BacklogList({
  items,
  heading,
  listLabel,
  optionPrefix,
  labelById,
  todayKey,
  showDeadline = false,
}: BacklogListProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const { tasks, taskListById } = useTasks();
  const { openTaskDialog, openPlanTask, invalidateData } = useDialogState();
  const { sectionColorById } = useCalendarStore();
  const { openForTask } = useChipContextMenu();
  const { visualEffortSizing } = useTaskCascadeEnabled();
  const [activeIndex, setActiveIndex] = useState(0);
  const headingId = useId();

  // Clamp the active option to the current list (it shrinks as tasks are
  // scheduled away). Derived, so it never points past the end.
  const activeIdx =
    items.length === 0 ? -1 : Math.min(activeIndex, items.length - 1);
  const optionId = (i: number) => `${optionPrefix}-opt-${i}`;

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (activeIdx < 0) return;
    const task = items[activeIdx];
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActiveIndex(Math.min(activeIdx + 1, items.length - 1));
        return;
      case 'ArrowUp':
        e.preventDefault();
        setActiveIndex(Math.max(activeIdx - 1, 0));
        return;
      case 'Home':
        e.preventDefault();
        setActiveIndex(0);
        return;
      case 'End':
        e.preventDefault();
        setActiveIndex(items.length - 1);
        return;
      case 'Enter':
      case ' ':
        e.preventDefault();
        openTaskDialog(task);
        return;
      case 'D':
      case 'd':
        if (e.shiftKey) {
          e.preventDefault();
          openPlanTask(task);
        } else if (e.ctrlKey || e.metaKey) {
          // Ctrl+D duplicates in place, matching TaskView.
          e.preventDefault();
          void duplicateTask(task).then(() => {
            invalidateData();
            announce(t('actions.duplicated', { title: task.title }));
          });
        }
        return;
      case 'ContextMenu':
      case 'F10': {
        if (e.key === 'F10' && !e.shiftKey) return;
        e.preventDefault();
        const node = document.getElementById(optionId(activeIdx));
        const rect = node?.getBoundingClientRect();
        void openForTask(
          task,
          rect ? { x: rect.left, y: rect.bottom } : undefined,
        );
        return;
      }
      default:
        return;
    }
  };

  return (
    <div className="backlog-rail__section">
      <h3 id={headingId} className="backlog-rail__subheading">
        {heading}
      </h3>
      <ul
        className="backlog-rail__list"
        role="listbox"
        aria-labelledby={headingId}
        aria-label={listLabel}
        tabIndex={0}
        aria-activedescendant={optionId(activeIdx)}
        onKeyDown={onListKeyDown}
      >
        {items.map((task, i) => {
          const children = tasks.filter((c) => c.parent_id === task.id);
          const listName = taskListById.get(task.list_id)?.name ?? task.list_id;
          const color = resolveTaskColor(
            task,
            taskListById,
            labelById,
            sectionColorById,
          );
          const priorityGlyph = priorityMarker(task.priority);
          const effortMod = visualEffortSizing
            ? effortSizeModifier(task.effort)
            : '';
          const parentTitle = subtaskParentTitle(task, tasks);
          const due =
            showDeadline && task.deadline_date
              ? fmt.format(parseDayKey(task.deadline_date), 'P')
              : null;
          const overdue =
            showDeadline &&
            !!task.deadline_date &&
            task.deadline_date < todayKey;
          const ariaLabel =
            showDeadline && due
              ? t('views.backlog.deadlineChipLabel', {
                  title: task.title,
                  list: listName,
                  deadline: due,
                  overdue: overdue ? t('views.backlog.overdueSuffix') : '',
                  priority: prioritySuffix(t, task.priority),
                  assignee: assigneeSuffix(t, task.assignees),
                })
              : t('views.backlog.chipLabel', {
                  title: task.title,
                  list: listName,
                  priority: prioritySuffix(t, task.priority),
                  assignee: assigneeSuffix(t, task.assignees),
                });
          return (
            <li
              key={task.id}
              id={optionId(i)}
              role="option"
              aria-selected={i === activeIdx}
              aria-label={
                ariaLabel +
                (parentTitle
                  ? t('views.tasks.subtaskParent', { parent: parentTitle })
                  : '') +
                effortSuffix(t, task.effort)
              }
              className={
                'backlog-rail__chip' +
                (i === activeIdx ? ' backlog-rail__chip--active' : '') +
                (overdue ? ' backlog-rail__chip--overdue' : '') +
                (effortMod ? ` backlog-rail__chip--effort-${effortMod}` : '')
              }
              style={
                color.hex
                  ? ({ '--event-color': color.hex } as React.CSSProperties)
                  : undefined
              }
              draggable
              onDragStart={(e) => setTaskDrag(e.dataTransfer, task, children)}
              onClick={() => setActiveIndex(i)}
              onDoubleClick={() => {
                setActiveIndex(i);
                openTaskDialog(task);
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                setActiveIndex(i);
                void openForTask(task, { x: e.clientX, y: e.clientY });
              }}
            >
              <span className="backlog-rail__chip-main">
                <span className="backlog-rail__chip-title">{task.title}</span>
                {priorityGlyph && (
                  <span
                    className="backlog-rail__chip-priority"
                    aria-hidden="true"
                  >
                    {priorityGlyph}
                  </span>
                )}
              </span>
              {parentTitle && (
                <span className="backlog-rail__chip-parent" aria-hidden="true">
                  ↳ {parentTitle}
                </span>
              )}
              <span className="backlog-rail__chip-meta">
                {due && (
                  <span
                    className={
                      'backlog-rail__chip-deadline' +
                      (overdue ? ' backlog-rail__chip-deadline--overdue' : '')
                    }
                    aria-hidden="true"
                  >
                    {due}
                  </span>
                )}
                <span className="backlog-rail__chip-list">{listName}</span>
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
