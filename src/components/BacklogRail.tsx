import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { isCommandError } from '../api/client';
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
 *   - **Drag source** — each chip is draggable; drop it on a day cell to
 *     schedule it there (the planner's existing day-drop handles it).
 *   - **Drop target** — drop a *scheduled* task onto the rail to send it
 *     back to the backlog (clears the date/deadline).
 *   - **Keyboard / screen-reader** — chips are real buttons: Enter opens
 *     the task, Shift+D opens the plan dialog (schedule it), the context
 *     menu offers the rest. So the rail is fully usable without a mouse.
 *
 * A native `<details>` keeps it a collapsible, accessible disclosure with
 * no custom widget plumbing. Rendered identically in the week and month
 * views, so the backlog travels with the planner.
 */
export function BacklogRail() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { tasks, taskListById } = useTasks();
  const { openTaskDialog, openPlanTask, invalidateData } = useDialogState();
  const { openForTask } = useChipContextMenu();
  const [open, setOpen] = useState(true);

  const backlog = useMemo(() => {
    const ids = new Set(tasks.map((row) => row.id));
    return tasks.filter(
      (row) =>
        row.status !== 'completed' &&
        !row.scheduled_date &&
        !row.deadline_date &&
        // top-level (or orphaned) — subtasks travel with their parent
        (!row.parent_id || !ids.has(row.parent_id)),
    );
  }, [tasks]);

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

  return (
    <details
      className="backlog-rail"
      open={open}
      onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(TASK_DND_TYPE)) return;
        // A task dragged over the rail → valid "back to backlog" drop.
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
      }}
      onDrop={(e) => void dropToBacklog(e)}
    >
      <summary className="backlog-rail__summary">
        {t('views.backlog.heading', { count: backlog.length })}
      </summary>
      <div className="backlog-rail__body">
        {backlog.length === 0 ? (
          <p className="backlog-rail__empty">{t('views.backlog.empty')}</p>
        ) : (
          <ul className="backlog-rail__list" role="list">
            {backlog.map((task) => {
              const children = tasks.filter((c) => c.parent_id === task.id);
              const listName =
                taskListById.get(task.list_id)?.name ?? task.list_id;
              return (
                <li key={task.id}>
                  <button
                    type="button"
                    className="backlog-rail__chip"
                    draggable
                    aria-label={t('views.backlog.chipLabel', {
                      title: task.title,
                      list: listName,
                    })}
                    onDragStart={(e) =>
                      setTaskDrag(e.dataTransfer, task, children)
                    }
                    onClick={() => openTaskDialog(task)}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      void openForTask(task);
                    }}
                    onKeyDown={(e) => {
                      // Shift+D → plan dialog (schedule), matching the
                      // task views. Stop propagation so the parent view's
                      // own key handler doesn't double-fire.
                      if (e.shiftKey && (e.key === 'D' || e.key === 'd')) {
                        e.preventDefault();
                        e.stopPropagation();
                        openPlanTask(task);
                      }
                    }}
                  >
                    <span className="backlog-rail__chip-title">
                      {task.title}
                    </span>
                    <span className="backlog-rail__chip-list">{listName}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </details>
  );
}
