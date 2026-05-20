import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../../a11y/Announcer';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveTaskColor } from '../../intl/eventColor';
import { useCalendarStore } from '../../state/CalendarStore';
import { useDialogState } from '../../state/DialogState';
import { useTasks } from '../../state/useTasks';
import type { Task, TaskStatus } from '../../api/types';
import { duplicateTask } from '../MoveCopyDialog';
import { ConfirmDialog } from '../ConfirmDialog';
import { isCommandError } from '../../api/client';

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
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);
  const { openTaskDialog, openMoveCopy } = useDialogState();

  // Flatten the task buckets into a single options array, interleaved
  // with separator entries. focusIndex points at the *task* index in
  // `flatTasks` — separators never receive focus.
  const { entries, flatTasks } = useMemo(
    () => buildEntries(tasks, taskListById, t),
    [tasks, taskListById, t],
  );

  const [focusIndex, setFocusIndex] = useState(0);

  useEffect(() => {
    if (focusIndex >= flatTasks.length) {
      setFocusIndex(Math.max(0, flatTasks.length - 1));
    }
  }, [flatTasks.length, focusIndex]);

  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-item-${i}`;
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const [confirmTarget, setConfirmTarget] = useState<Task | null>(null);
  const performDelete = useCallback(
    async (task: Task) => {
      try {
        await invoke<void>('delete_task', {
          id: task.id,
          listId: task.list_id,
        });
        announce(t('dialogs.task.deleted', { title: task.title }));
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t],
  );

  const toggleStatus = useCallback(
    async (task: Task) => {
      const nextStatus: TaskStatus =
        task.status === 'completed' ? 'open' : 'completed';
      const updated: Task = {
        ...task,
        status: nextStatus,
        completed_at:
          nextStatus === 'completed' ? new Date().toISOString() : null,
      };
      try {
        await invoke<Task>('update_task', { task: updated });
        announce(
          nextStatus === 'completed'
            ? t('views.tasks.completedAnnounce', { title: task.title })
            : t('views.tasks.reopenedAnnounce', { title: task.title }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('update_task failed', err);
      }
    },
    [announce, t],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
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
      if (flatTasks.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, flatTasks.length - 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          setFocusIndex((i) => Math.max(i - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          setFocusIndex(0);
          return;
        case 'End':
          e.preventDefault();
          setFocusIndex(flatTasks.length - 1);
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
      announce,
      t,
    ],
  );

  return (
    <section className="view view--tasks" aria-label={t('views.tasks.title')}>
      <header className="view__header">
        <h2>{t('views.tasks.title')}</h2>
      </header>

      <ul
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.tasks.taskList')}
        aria-activedescendant={
          flatTasks.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="task-list"
      >
        {flatTasks.length === 0 && (
          <li role="presentation" className="task-list__empty">
            {t('views.tasks.empty')}
          </li>
        )}
        {entries.map((entry) => {
          if (entry.kind === 'separator') {
            return (
              <li
                key={`sep-${entry.label}`}
                role="presentation"
                aria-hidden="true"
                className="task-list__group"
              >
                {entry.label}
              </li>
            );
          }
          const { task, listName, index } = entry;
          const focused = index === focusIndex;
          const checked = task.status === 'completed';
          const due = describeDue(task, fmt, t);
          const color = resolveTaskColor(task, taskListById, labelById);
          const aria = color.labelName
            ? t('views.tasks.optionLabelWithLabel', {
                title: task.title,
                list: listName,
                state: checked
                  ? t('views.tasks.stateDone')
                  : t('views.tasks.stateOpen'),
                due,
                label: color.labelName,
              })
            : t('views.tasks.optionLabel', {
                title: task.title,
                list: listName,
                state: checked
                  ? t('views.tasks.stateDone')
                  : t('views.tasks.stateOpen'),
                due,
              });
          return (
            <li
              key={task.id}
              id={itemId(index)}
              role="option"
              aria-selected={focused}
              aria-label={aria}
              className={
                'task-list__item' +
                (focused ? ' task-list__item--focused' : '') +
                (checked ? ' task-list__item--done' : '')
              }
              style={
                color.hex
                  ? ({ '--event-color': color.hex } as React.CSSProperties)
                  : undefined
              }
              onClick={() => {
                setFocusIndex(index);
                void toggleStatus(task);
              }}
            >
              <span className="task-list__check" aria-hidden="true">
                {checked ? '☑' : '☐'}
              </span>
              <span className="task-list__title">{task.title}</span>
              <span className="task-list__due">{due}</span>
            </li>
          );
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
    </section>
  );
}

type Entry =
  | { kind: 'separator'; label: string }
  | { kind: 'task'; task: Task; listName: string; index: number };

function buildEntries(
  tasks: Task[],
  taskListById: Map<string, { name: string }>,
  t: (key: string, vars?: Record<string, unknown>) => string,
): { entries: Entry[]; flatTasks: Task[] } {
  const backlog: Task[] = [];
  const byList = new Map<string, Task[]>();
  tasks.forEach((task) => {
    if (!task.scheduled_date && !task.deadline_date) {
      backlog.push(task);
      return;
    }
    const bucket = byList.get(task.list_id) ?? [];
    bucket.push(task);
    byList.set(task.list_id, bucket);
  });

  const sortedLists = Array.from(byList.entries()).sort(([a], [b]) =>
    a.localeCompare(b),
  );

  const entries: Entry[] = [];
  const flatTasks: Task[] = [];

  const push = (task: Task, listName: string) => {
    entries.push({ kind: 'task', task, listName, index: flatTasks.length });
    flatTasks.push(task);
  };

  if (backlog.length > 0) {
    entries.push({ kind: 'separator', label: t('views.tasks.backlog') });
    backlog.forEach((task) =>
      push(task, taskListById.get(task.list_id)?.name ?? task.list_id),
    );
  }

  sortedLists.forEach(([listId, items]) => {
    const name = taskListById.get(listId)?.name ?? listId;
    entries.push({ kind: 'separator', label: name });
    items.forEach((task) => push(task, name));
  });

  return { entries, flatTasks };
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
