import { useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { isCommandError } from '../api/client';
import { labelsLookup, resolveTaskColor } from '../intl/eventColor';
import {
  priorityMarker,
  priorityRank,
  prioritySuffix,
} from '../intl/taskStatus';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import {
  moveTaskToBacklog,
  readTaskDrag,
  setTaskDrag,
  TASK_DND_TYPE,
} from '../state/moveActions';
import { useChipContextMenu } from '../state/useChipContextMenu';
import { useTasks } from '../state/useTasks';

/**
 * Backlog rail for the week / month planner.
 *
 * Lists the unscheduled backlog — open, top-level tasks with no
 * `scheduled_date` and no `deadline_date` (the same bucket the task view's
 * "Backlog" group shows). Three roles in one:
 *
 *   - **Drag source** — each option is draggable; drop it on a day cell to
 *     schedule it there (the planner's existing day-drop handles it).
 *   - **Drop target** — drop a *scheduled* task onto the rail to send it
 *     back to the backlog (clears the date/deadline).
 *   - **Keyboard / screen-reader** — a single-tab-stop `listbox`: Tab lands
 *     on the list once, Arrow/Home/End move the active option (via
 *     `aria-activedescendant`, not one tab-stop per task), Enter opens it,
 *     Shift+D opens the plan dialog (schedule), ContextMenu / Shift+F10
 *     opens the task menu. So the rail is fully usable without a mouse.
 *
 * Rendered as a fixed-width vertical list to the left of the week / month
 * grid; it joins the F6 region cycle (via `data-region`). Each chip carries
 * its hierarchical color (task → section → list) and a priority marker, and
 * the list is ordered high → low priority.
 */
export function BacklogRail() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { tasks, taskListById } = useTasks();
  const { openTaskDialog, openPlanTask, invalidateData } = useDialogState();
  const { openForTask } = useChipContextMenu();
  const { colorLabels, sectionColorById, sectionsByList, loadSections } =
    useCalendarStore();
  const [activeIndex, setActiveIndex] = useState(0);
  const headingId = useId();

  const backlog = useMemo(() => {
    const ids = new Set(tasks.map((row) => row.id));
    return tasks
      .filter(
        (row) =>
          row.status !== 'completed' &&
          !row.scheduled_date &&
          !row.deadline_date &&
          // top-level (or orphaned) — subtasks travel with their parent
          (!row.parent_id || !ids.has(row.parent_id)),
      )
      // High priority to the top, low to the bottom (stable → existing order
      // is the tiebreaker within one priority band).
      .sort((a, b) => priorityRank(a.priority) - priorityRank(b.priority));
  }, [tasks]);

  // Per-chip color follows the task → section → list chain. Sections load
  // lazily, so trigger a load for every list that has a backlog task.
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);
  const listIds = useMemo(
    () => Array.from(new Set(backlog.map((task) => task.list_id))),
    [backlog],
  );
  useEffect(() => {
    for (const listId of listIds) {
      if (!(listId in sectionsByList)) void loadSections(listId);
    }
  }, [listIds, sectionsByList, loadSections]);

  // Clamp the active option to the current list (it shrinks as tasks are
  // scheduled away). Derived, so it never points past the end.
  const activeIdx =
    backlog.length === 0 ? -1 : Math.min(activeIndex, backlog.length - 1);
  const optionId = (i: number) => `backlog-opt-${i}`;

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

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (activeIdx < 0) return;
    const task = backlog[activeIdx];
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActiveIndex(Math.min(activeIdx + 1, backlog.length - 1));
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
        setActiveIndex(backlog.length - 1);
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
    <section
      className="backlog-rail"
      data-region="backlog"
      aria-labelledby={headingId}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(TASK_DND_TYPE)) return;
        // A task dragged over the rail → valid "back to backlog" drop.
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
      }}
      onDrop={(e) => void dropToBacklog(e)}
    >
      <h2 id={headingId} className="backlog-rail__heading">
        {t('views.backlog.heading', { count: backlog.length })}
      </h2>
      <div className="backlog-rail__body">
        {backlog.length === 0 ? (
          <p className="backlog-rail__empty">{t('views.backlog.empty')}</p>
        ) : (
          <ul
            className="backlog-rail__list"
            role="listbox"
            aria-label={t('views.backlog.listLabel')}
            tabIndex={0}
            aria-activedescendant={optionId(activeIdx)}
            onKeyDown={onListKeyDown}
          >
            {backlog.map((task, i) => {
              const children = tasks.filter((c) => c.parent_id === task.id);
              const listName =
                taskListById.get(task.list_id)?.name ?? task.list_id;
              const color = resolveTaskColor(
                task,
                taskListById,
                labelById,
                sectionColorById,
              );
              const priorityGlyph = priorityMarker(task.priority);
              return (
                <li
                  key={task.id}
                  id={optionId(i)}
                  role="option"
                  aria-selected={i === activeIdx}
                  aria-label={t('views.backlog.chipLabel', {
                    title: task.title,
                    list: listName,
                    priority: prioritySuffix(t, task.priority),
                  })}
                  className={
                    'backlog-rail__chip' +
                    (i === activeIdx ? ' backlog-rail__chip--active' : '')
                  }
                  style={
                    color.hex
                      ? ({ '--event-color': color.hex } as React.CSSProperties)
                      : undefined
                  }
                  draggable
                  onDragStart={(e) => setTaskDrag(e.dataTransfer, task, children)}
                  onClick={() => {
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
                    <span className="backlog-rail__chip-title">
                      {task.title}
                    </span>
                    {priorityGlyph && (
                      <span
                        className="backlog-rail__chip-priority"
                        aria-hidden="true"
                      >
                        {priorityGlyph}
                      </span>
                    )}
                  </span>
                  <span className="backlog-rail__chip-list">{listName}</span>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}
