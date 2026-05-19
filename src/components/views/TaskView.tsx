import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../../a11y/Announcer';
import { useDateFormat } from '../../intl/dateFormat';
import { useTasks } from '../../state/useTasks';
import type { Task, TaskStatus } from '../../api/types';

/**
 * Dedicated task view (DESIGN.md section 9.8).
 *
 * Phase 3 ships the structural skeleton:
 *  - Backlog group (tasks without a scheduled date or deadline).
 *  - Per-list groupings.
 *  - Completed tasks hidden by default — toggled via the header button.
 *  - `Space` on a focused row toggles completion (the global shortcut
 *    handler is added in Phase 5; today the row's button handles it).
 *
 * Filtering, sorting, and the Wochenplanung Drag&Drop come with Phase 4.
 */
export function TaskView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { tasks, taskListById, loading } = useTasks();

  const { backlog, grouped } = useMemo(() => splitByList(tasks), [tasks]);

  const toggleStatus = useCallback(
    async (task: Task) => {
      const nextStatus: TaskStatus =
        task.status === 'completed' ? 'open' : 'completed';
      const updated: Task = {
        ...task,
        status: nextStatus,
        completed_at: nextStatus === 'completed' ? new Date().toISOString() : null,
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

  return (
    <section className="view view--tasks" aria-label={t('views.tasks.title')}>
      <header className="view__header">
        <h2>{t('views.tasks.title')}</h2>
      </header>

      {loading && <p>{t('views.loading')}</p>}
      {!loading && tasks.length === 0 && <p>{t('views.tasks.empty')}</p>}

      {backlog.length > 0 && (
        <section
          className="task-group"
          aria-label={t('views.tasks.backlog')}
        >
          <h3>{t('views.tasks.backlog')}</h3>
          <ul role="list">
            {backlog.map((task) => (
              <TaskRow
                key={task.id}
                task={task}
                listName={taskListById.get(task.list_id)?.name ?? '—'}
                onToggle={toggleStatus}
                fmt={fmt}
              />
            ))}
          </ul>
        </section>
      )}

      {grouped.map(({ listId, tasks: items }) => {
        const list = taskListById.get(listId);
        return (
          <section
            key={listId}
            className="task-group"
            aria-label={list?.name ?? listId}
          >
            <h3>{list?.name ?? listId}</h3>
            <ul role="list">
              {items.map((task) => (
                <TaskRow
                  key={task.id}
                  task={task}
                  listName={list?.name ?? listId}
                  onToggle={toggleStatus}
                  fmt={fmt}
                />
              ))}
            </ul>
          </section>
        );
      })}
    </section>
  );
}

interface TaskRowProps {
  task: Task;
  listName: string;
  onToggle: (task: Task) => void;
  fmt: ReturnType<typeof useDateFormat>;
}

function TaskRow({ task, listName, onToggle, fmt }: TaskRowProps) {
  const { t } = useTranslation();
  const checked = task.status === 'completed';

  const dueLabel = useMemo(() => {
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
  }, [task.scheduled_date, task.deadline_date, fmt, t]);

  return (
    <li role="listitem" className={'task-row' + (checked ? ' task-row--done' : '')}>
      <button
        type="button"
        role="checkbox"
        aria-checked={checked}
        aria-label={t('views.tasks.toggleLabel', {
          title: task.title,
          list: listName,
        })}
        onClick={() => onToggle(task)}
        className="task-row__check"
      >
        {checked ? '☑' : '☐'}
      </button>
      <span className="task-row__title">{task.title}</span>
      <span className="task-row__due">{dueLabel}</span>
    </li>
  );
}

interface GroupedList {
  listId: string;
  tasks: Task[];
}

function splitByList(tasks: Task[]): {
  backlog: Task[];
  grouped: GroupedList[];
} {
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

  const grouped: GroupedList[] = [];
  byList.forEach((items, listId) => {
    grouped.push({ listId, tasks: items });
  });
  // Stable order: by list id; the store's list lookup gives us human
  // names for display, but the bucket order itself is deterministic.
  grouped.sort((a, b) => a.listId.localeCompare(b.listId));
  return { backlog, grouped };
}
