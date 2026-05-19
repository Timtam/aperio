import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createTask as apiCreateTask,
  isCommandError,
} from '../api/client';
import { invoke } from '@tauri-apps/api/core';
import type { DeadlineType, Task, TaskPriority, TaskStatus } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { Modal } from './Modal';
import {
  fromBackend as recurrenceFromBackend,
  toBackend as recurrenceToBackend,
  TaskRecurrenceSelector,
  TASK_RECURRENCE_DEFAULT,
  type TaskRecurrenceValue,
} from './TaskRecurrenceSelector';

/**
 * Task create / edit dialog (DESIGN.md section 9.9).
 *
 * Phase 4a fields: title, list, status, priority, deadline type
 * (none/scheduled/on/by), date, optional time, description. Sub-tasks,
 * recurrence, color labels, reminders, and sound overrides land in
 * later waves.
 */
export interface TaskDialogProps {
  isOpen: boolean;
  onClose: () => void;
  task: Task | null;
  /** Pre-selected list when creating a new task. */
  defaultListId?: string;
}

type DeadlineMode = 'none' | 'scheduled' | 'on' | 'by';

interface FormState {
  title: string;
  listId: string;
  status: TaskStatus;
  priority: TaskPriority;
  deadlineMode: DeadlineMode;
  date: string;
  time: string;
  description: string;
  colorLabel: string | null;
  recurrence: TaskRecurrenceValue;
}

export function TaskDialog({
  isOpen,
  onClose,
  task,
  defaultListId,
}: TaskDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { taskLists, colorLabels } = useCalendarStore();

  const isEdit = task !== null;
  const initialState = useMemo<FormState>(
    () => buildInitialState(task, defaultListId, taskLists),
    [task, defaultListId, taskLists],
  );

  const [form, setForm] = useState<FormState>(initialState);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setForm(initialState);
      setError(null);
    }
  }, [isOpen, initialState]);

  const update = useCallback(
    <K extends keyof FormState>(key: K, value: FormState[K]) => {
      setForm((prev) => ({ ...prev, [key]: value }));
    },
    [],
  );

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      setError(null);

      const trimmedTitle = form.title.trim();
      if (!trimmedTitle) {
        setError(t('dialogs.task.titleRequired'));
        return;
      }
      if (!form.listId) {
        setError(t('dialogs.task.listRequired'));
        return;
      }

      const { scheduled_date, deadline_type, deadline_date, deadline_time } =
        splitDeadline(form);

      setSubmitting(true);
      try {
        if (isEdit && task) {
          const updated: Task = {
            ...task,
            title: trimmedTitle,
            list_id: form.listId,
            status: form.status,
            priority: form.priority,
            scheduled_date,
            deadline_type,
            deadline_date,
            deadline_time,
            recurrence: recurrenceToBackend(form.recurrence),
            description: form.description.trim() || null,
            color_label: form.colorLabel,
            completed_at:
              form.status === 'completed'
                ? task.completed_at ?? new Date().toISOString()
                : null,
          };
          await invoke<Task>('update_task', { task: updated });
          announce(t('dialogs.task.updated', { title: trimmedTitle }));
        } else {
          await apiCreateTask({
            list_id: form.listId,
            title: trimmedTitle,
            description: form.description.trim() || null,
            status: form.status,
            priority: form.priority,
            scheduled_date,
            deadline_type,
            deadline_date,
            deadline_time,
            recurrence: recurrenceToBackend(form.recurrence),
            parent_id: null,
            color_label: form.colorLabel,
            reminders: [],
            sound: null,
          });
          announce(t('dialogs.task.created', { title: trimmedTitle }));
        }
        onClose();
      } catch (err) {
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      } finally {
        setSubmitting(false);
      }
    },
    [form, isEdit, task, announce, onClose, t],
  );

  const onDelete = useCallback(async () => {
    if (!task) return;
    setError(null);
    setSubmitting(true);
    try {
      await invoke<void>('delete_task', { id: task.id });
      announce(t('dialogs.task.deleted', { title: task.title }));
      onClose();
    } catch (err) {
      if (isCommandError(err)) {
        setError(`${err.code}: ${err.message}`);
      } else {
        setError(String(err));
      }
    } finally {
      setSubmitting(false);
    }
  }, [task, announce, onClose, t]);

  const title = isEdit
    ? t('dialogs.task.editTitle')
    : t('dialogs.task.newTitle');

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      className="modal--form"
      dismissOnBackdrop={false}
    >
      <form onSubmit={onSubmit} className="form">
        <label className="form__field">
          <span className="form__label">{t('dialogs.task.fields.title')}</span>
          <input
            type="text"
            value={form.title}
            onChange={(e) => update('title', e.target.value)}
            required
            autoComplete="off"
          />
        </label>

        <label className="form__field">
          <span className="form__label">{t('dialogs.task.fields.list')}</span>
          <select
            value={form.listId}
            onChange={(e) => update('listId', e.target.value)}
            required
          >
            <option value="" disabled>
              {t('dialogs.task.pickList')}
            </option>
            {taskLists.map((list) => (
              <option key={list.id} value={list.id}>
                {list.name}
              </option>
            ))}
          </select>
        </label>

        <div className="form__row">
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.task.fields.status')}
            </span>
            <select
              value={form.status}
              onChange={(e) =>
                update('status', e.target.value as TaskStatus)
              }
            >
              <option value="open">{t('dialogs.task.status.open')}</option>
              <option value="in_progress">
                {t('dialogs.task.status.inProgress')}
              </option>
              <option value="completed">
                {t('dialogs.task.status.completed')}
              </option>
              <option value="cancelled">
                {t('dialogs.task.status.cancelled')}
              </option>
            </select>
          </label>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.task.fields.priority')}
            </span>
            <select
              value={form.priority}
              onChange={(e) =>
                update('priority', e.target.value as TaskPriority)
              }
            >
              <option value="low">{t('dialogs.task.priority.low')}</option>
              <option value="medium">
                {t('dialogs.task.priority.medium')}
              </option>
              <option value="high">{t('dialogs.task.priority.high')}</option>
            </select>
          </label>
        </div>

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.task.fields.deadlineMode')}
          </span>
          <select
            value={form.deadlineMode}
            onChange={(e) =>
              update('deadlineMode', e.target.value as DeadlineMode)
            }
          >
            <option value="none">{t('dialogs.task.deadline.none')}</option>
            <option value="scheduled">
              {t('dialogs.task.deadline.scheduled')}
            </option>
            <option value="on">{t('dialogs.task.deadline.on')}</option>
            <option value="by">{t('dialogs.task.deadline.by')}</option>
          </select>
        </label>

        {form.deadlineMode !== 'none' && (
          <div className="form__row">
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.fields.date')}
              </span>
              <input
                type="date"
                value={form.date}
                onChange={(e) => update('date', e.target.value)}
                required
              />
            </label>
            {form.deadlineMode === 'on' && (
              <label className="form__field">
                <span className="form__label">
                  {t('dialogs.task.fields.time')}
                </span>
                <input
                  type="time"
                  value={form.time}
                  onChange={(e) => update('time', e.target.value)}
                />
              </label>
            )}
          </div>
        )}

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.task.fields.description')}
          </span>
          <textarea
            value={form.description}
            onChange={(e) => update('description', e.target.value)}
            rows={4}
          />
        </label>

        <TaskRecurrenceSelector
          value={form.recurrence}
          onChange={(recurrence) => update('recurrence', recurrence)}
        />

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.task.fields.colorLabel')}
          </span>
          <select
            value={form.colorLabel ?? ''}
            onChange={(e) =>
              update('colorLabel', e.target.value ? e.target.value : null)
            }
          >
            <option value="">{t('dialogs.task.noColorLabel')}</option>
            {colorLabels.map((label) => (
              <option key={label.id} value={label.id}>
                {label.name}
              </option>
            ))}
          </select>
        </label>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <div className="form__actions">
          {isEdit && (
            <button
              type="button"
              onClick={onDelete}
              disabled={submitting}
              className="form__action form__action--danger"
            >
              {t('dialogs.task.delete')}
            </button>
          )}
          <button
            type="button"
            onClick={onClose}
            disabled={submitting}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            disabled={submitting}
            className="form__action form__action--primary"
          >
            {isEdit ? t('dialogs.save') : t('dialogs.create')}
          </button>
        </div>
      </form>
    </Modal>
  );
}

function buildInitialState(
  task: Task | null,
  defaultListId: string | undefined,
  taskLists: { id: string }[],
): FormState {
  if (task) {
    const mode: DeadlineMode = task.scheduled_date
      ? 'scheduled'
      : task.deadline_type === 'on'
      ? 'on'
      : task.deadline_type === 'by'
      ? 'by'
      : 'none';
    return {
      title: task.title,
      listId: task.list_id,
      status: task.status,
      priority: task.priority,
      deadlineMode: mode,
      date:
        task.scheduled_date ??
        task.deadline_date ??
        todayInput(),
      time: task.deadline_time?.slice(0, 5) ?? '09:00',
      description: task.description ?? '',
      colorLabel: task.color_label ?? null,
      recurrence: recurrenceFromBackend(task.recurrence),
    };
  }
  return {
    title: '',
    listId: defaultListId ?? taskLists[0]?.id ?? '',
    status: 'open',
    priority: 'medium',
    deadlineMode: 'none',
    date: todayInput(),
    time: '09:00',
    description: '',
    colorLabel: null,
    recurrence: { ...TASK_RECURRENCE_DEFAULT },
  };
}

interface DeadlineFields {
  scheduled_date: string | null;
  deadline_type: DeadlineType | null;
  deadline_date: string | null;
  deadline_time: string | null;
}

function splitDeadline(form: FormState): DeadlineFields {
  switch (form.deadlineMode) {
    case 'scheduled':
      return {
        scheduled_date: form.date || null,
        deadline_type: null,
        deadline_date: null,
        deadline_time: null,
      };
    case 'on':
      return {
        scheduled_date: null,
        deadline_type: 'on',
        deadline_date: form.date || null,
        deadline_time: form.time ? `${form.time}:00` : null,
      };
    case 'by':
      return {
        scheduled_date: null,
        deadline_type: 'by',
        deadline_date: form.date || null,
        deadline_time: null,
      };
    default:
      return {
        scheduled_date: null,
        deadline_type: null,
        deadline_date: null,
        deadline_time: null,
      };
  }
}

function todayInput(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}
