import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createTask as apiCreateTask,
  isCommandError,
  showContextMenu,
  type ContextMenuItemRequest,
} from '../api/client';
import { invoke } from '@tauri-apps/api/core';
import { statusI18nKey, statusMarker } from '../intl/taskStatus';
import type {
  DeadlineType,
  Reminder,
  Task,
  TaskPriority,
  TaskStatus,
} from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { useDialogState } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { Modal } from './Modal';
import { RemindersEditor } from './RemindersEditor';
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
  /** ISO date used to pre-fill scheduled_date when creating. */
  defaultDate?: string;
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
  reminders: Reminder[];
}

export function TaskDialog({
  isOpen,
  onClose,
  task,
  defaultListId,
  defaultDate,
}: TaskDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { taskLists, colorLabels } = useCalendarStore();
  const { tasks } = useTasks();
  const { invalidateData } = useDialogState();

  const isEdit = task !== null;

  // Subtasks: children of the task currently being edited. Only
  // meaningful in edit mode — a brand-new task has no id yet, so
  // children can't reference it. Filtering here over the global
  // task list keeps the dialog in sync with TaskView whenever the
  // user (or a sync) mutates a child elsewhere.
  const subtasks = useMemo<Task[]>(() => {
    if (!task) return [];
    return tasks.filter((row) => row.parent_id === task.id);
  }, [tasks, task]);

  const [newSubtaskTitle, setNewSubtaskTitle] = useState('');
  const [subtaskBusy, setSubtaskBusy] = useState(false);
  // Listbox focus: aria-activedescendant index into `subtasks`. The
  // listbox owns the single tab stop; arrow keys move this counter
  // and the option with the matching id lights up as the focused
  // descendant — same pattern TaskView uses.
  const [focusedSubtaskIdx, setFocusedSubtaskIdx] = useState(0);
  const subtaskListId = useId();
  const subtaskItemId = useCallback(
    (i: number) => `${subtaskListId}-item-${i}`,
    [subtaskListId],
  );

  useEffect(() => {
    if (focusedSubtaskIdx >= subtasks.length) {
      setFocusedSubtaskIdx(Math.max(0, subtasks.length - 1));
    }
  }, [subtasks.length, focusedSubtaskIdx]);
  const initialState = useMemo<FormState>(
    () => buildInitialState(task, defaultListId, defaultDate, taskLists),
    [task, defaultListId, defaultDate, taskLists],
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

  // Subtask mutations apply immediately (not staged with the parent
  // form's Save) so the user can check things off without losing
  // unsaved parent-field edits. Each path bumps dataVersion so
  // useTasks refetches and the inline list refreshes.
  const addSubtask = useCallback(async () => {
    if (!task) return;
    const trimmed = newSubtaskTitle.trim();
    if (!trimmed || subtaskBusy) return;
    setSubtaskBusy(true);
    try {
      await apiCreateTask({
        list_id: task.list_id,
        title: trimmed,
        description: null,
        status: 'open',
        priority: 'medium',
        scheduled_date: null,
        deadline_type: null,
        deadline_date: null,
        deadline_time: null,
        recurrence: null,
        parent_id: task.id,
        color_label: null,
        reminders: [],
        sound: null,
      });
      setNewSubtaskTitle('');
      invalidateData();
      announce(t('dialogs.task.subtasks.added', { title: trimmed }));
    } catch (err) {
      // Show inline rather than steal the parent form's error slot.
      announce(
        isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
      );
    } finally {
      setSubtaskBusy(false);
    }
  }, [task, newSubtaskTitle, subtaskBusy, invalidateData, announce, t]);

  const toggleSubtaskStatus = useCallback(
    async (subtask: Task) => {
      const next: TaskStatus =
        subtask.status === 'completed' ? 'open' : 'completed';
      try {
        await invoke<Task>('update_task', {
          task: {
            ...subtask,
            status: next,
            completed_at:
              next === 'completed' ? new Date().toISOString() : null,
          },
        });
        invalidateData();
        announce(
          next === 'completed'
            ? t('views.tasks.completedAnnounce', { title: subtask.title })
            : t('views.tasks.reopenedAnnounce', { title: subtask.title }),
        );
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [invalidateData, announce, t],
  );

  const deleteSubtask = useCallback(
    async (subtask: Task) => {
      try {
        await invoke<void>('delete_task', {
          id: subtask.id,
          listId: subtask.list_id,
        });
        invalidateData();
        announce(t('dialogs.task.deleted', { title: subtask.title }));
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [invalidateData, announce, t],
  );

  const setSubtaskStatus = useCallback(
    async (subtask: Task, next: TaskStatus) => {
      if (subtask.status === next) return;
      try {
        await invoke<Task>('update_task', {
          task: {
            ...subtask,
            status: next,
            completed_at:
              next === 'completed' ? new Date().toISOString() : null,
          },
        });
        invalidateData();
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [invalidateData, announce],
  );

  // Per-subtask context menu (right-click + Shift+F10). Limited to
  // status + delete — opening a TaskDialog from inside another
  // TaskDialog would stack modals on the same surface, which is the
  // reason "edit" lives in TaskView instead. Status changes use
  // CheckMenuItems so the OS draws its own glyph on the active row.
  const openSubtaskMenu = useCallback(
    async (
      subtask: Task,
      position?: { x: number; y: number },
    ) => {
      const items: ContextMenuItemRequest[] = [
        {
          kind: 'submenu',
          label: t('chipMenu.status'),
          items: (
            ['open', 'in_progress', 'completed', 'cancelled'] as TaskStatus[]
          ).map((s) => ({
            kind: 'check' as const,
            id: `status:${s}`,
            label: t(`chipMenu.statusValue.${s}`),
            checked: subtask.status === s,
          })),
        },
        { kind: 'separator' },
        { id: 'delete', label: t('chipMenu.delete') },
      ];
      let selected: string | null = null;
      try {
        selected = await showContextMenu(items, position);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('show_context_menu failed', err);
      }
      if (selected?.startsWith('status:')) {
        await setSubtaskStatus(subtask, selected.slice('status:'.length) as TaskStatus);
      } else if (selected === 'delete') {
        await deleteSubtask(subtask);
      }
    },
    [t, setSubtaskStatus, deleteSubtask],
  );

  const onSubtaskListKey = useCallback(
    (e: ReactKeyboardEvent) => {
      if (subtasks.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusedSubtaskIdx((i) =>
            Math.min(i + 1, subtasks.length - 1),
          );
          return;
        case 'ArrowUp':
          e.preventDefault();
          setFocusedSubtaskIdx((i) => Math.max(i - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          setFocusedSubtaskIdx(0);
          return;
        case 'End':
          e.preventDefault();
          setFocusedSubtaskIdx(subtasks.length - 1);
          return;
        case ' ':
        case 'Spacebar': {
          // Space toggles open/completed — same Space-to-check
          // contract the rest of the app uses for tasks.
          e.preventDefault();
          const sub = subtasks[focusedSubtaskIdx];
          if (sub) void toggleSubtaskStatus(sub);
          return;
        }
        case 'ContextMenu':
        case 'F10': {
          if (e.key === 'F10' && !e.shiftKey) return;
          e.preventDefault();
          const sub = subtasks[focusedSubtaskIdx];
          if (sub) {
            const target = e.currentTarget as HTMLElement;
            const id = subtaskItemId(focusedSubtaskIdx);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            void openSubtaskMenu(
              sub,
              rect ? { x: rect.left, y: rect.bottom } : undefined,
            );
          }
          return;
        }
        default:
          return;
      }
    },
    [
      subtasks,
      focusedSubtaskIdx,
      toggleSubtaskStatus,
      openSubtaskMenu,
      subtaskItemId,
    ],
  );

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      if (submitting) return;
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
            reminders: form.reminders,
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
            reminders: form.reminders,
            sound: null,
          });
          // Remember for next time — only on create, not edit.
          writeLastUsedTaskList(form.listId);
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
    [form, submitting, isEdit, task, announce, onClose, t],
  );

  const onDelete = useCallback(async () => {
    if (!task) return;
    if (submitting) return;
    setError(null);
    setSubmitting(true);
    try {
      await invoke<void>('delete_task', {
        id: task.id,
        listId: task.list_id,
      });
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
  }, [task, submitting, announce, onClose, t]);

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

        <RemindersEditor
          value={form.reminders}
          onChange={(next) => update('reminders', next)}
          mode="task"
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

        {isEdit && task && (
          <fieldset className="form__field form__field--subtasks">
            <legend className="form__label">
              {t('dialogs.task.subtasks.heading')}
            </legend>
            {/* Subtasks section. Hidden on create — a brand new
                parent has no id, so children couldn't reference it.
                The user finishes the create flow first, then
                re-opens to add subtasks. Existing tasks see the
                list immediately and can edit it inline.
                Mutations here persist immediately (not staged with
                the parent form). That's a deliberate pick: the most
                common use is "tick a subtask off mid-edit", which
                shouldn't be coupled to whether the parent's other
                changes are saved yet. */}
            {subtasks.length === 0 ? (
              <p className="subtasks__empty">
                {t('dialogs.task.subtasks.empty')}
              </p>
            ) : (
              <ul
                role="listbox"
                tabIndex={0}
                className="subtasks__list"
                aria-label={t('dialogs.task.subtasks.listAria')}
                aria-activedescendant={subtaskItemId(focusedSubtaskIdx)}
                onKeyDown={onSubtaskListKey}
              >
                {/* One tab stop owns the whole list — focus moves
                    between options via aria-activedescendant. Each
                    row carries aria-checked so AT announces the
                    Space-to-toggle binding correctly. Delete and
                    Status changes live in the per-row context menu
                    (Shift+F10 / right-click). */}
                {subtasks.map((sub, i) => {
                  const focused = i === focusedSubtaskIdx;
                  const isCompleted = sub.status === 'completed';
                  const stateLabel = t(statusI18nKey(sub.status));
                  return (
                    <li
                      key={sub.id}
                      id={subtaskItemId(i)}
                      role="option"
                      aria-selected={focused}
                      aria-checked={isCompleted}
                      aria-label={t('dialogs.task.subtasks.rowLabel', {
                        title: sub.title,
                        state: stateLabel,
                      })}
                      className={
                        'subtasks__row' +
                        (focused ? ' subtasks__row--focused' : '')
                      }
                      onClick={() => {
                        setFocusedSubtaskIdx(i);
                        void toggleSubtaskStatus(sub);
                      }}
                      onContextMenu={(ev) => {
                        ev.preventDefault();
                        ev.stopPropagation();
                        setFocusedSubtaskIdx(i);
                        void openSubtaskMenu(sub);
                      }}
                    >
                      <span className="subtasks__check" aria-hidden="true">
                        {statusMarker(sub.status)}
                      </span>
                      <span
                        className={
                          'subtasks__title' +
                          (sub.status === 'completed' ||
                          sub.status === 'cancelled'
                            ? ' subtasks__title--done'
                            : '')
                        }
                      >
                        {sub.title}
                      </span>
                    </li>
                  );
                })}
              </ul>
            )}
            <div className="subtasks__add">
              <input
                type="text"
                value={newSubtaskTitle}
                onChange={(e) => setNewSubtaskTitle(e.target.value)}
                onKeyDown={(e) => {
                  // Enter inside this input adds the subtask without
                  // submitting the parent form — preventDefault
                  // stops the form-submit fallthrough.
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    void addSubtask();
                  }
                }}
                placeholder={t('dialogs.task.subtasks.placeholder')}
                aria-label={t('dialogs.task.subtasks.newAria')}
                disabled={subtaskBusy}
              />
              <button
                type="button"
                onClick={() => void addSubtask()}
                disabled={subtaskBusy || !newSubtaskTitle.trim()}
                className="subtasks__add-button"
              >
                {t('dialogs.task.subtasks.addButton')}
              </button>
            </div>
          </fieldset>
        )}

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
              aria-disabled={submitting || undefined}
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

/** Mirrors EventDialog's last-used-calendar memo. The task-list
 *  picker on a new task remembers the user's previous pick so a
 *  multi-list setup doesn't reset to `taskLists[0]` on every open. */
const LAST_USED_TASK_LIST_KEY = 'aperio.lastUsedTaskList.v1';

export function readLastUsedTaskList(): string | null {
  try {
    return localStorage.getItem(LAST_USED_TASK_LIST_KEY);
  } catch {
    return null;
  }
}

export function writeLastUsedTaskList(id: string): void {
  try {
    localStorage.setItem(LAST_USED_TASK_LIST_KEY, id);
  } catch {
    // Best effort.
  }
}

function buildInitialState(
  task: Task | null,
  defaultListId: string | undefined,
  defaultDate: string | undefined,
  taskLists: { id: string; read_only: boolean }[],
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
      reminders: task.reminders ?? [],
    };
  }
  // When the caller anchored us on a day, default to a "scheduled
  // task on that day". When unset, the task starts dateless (backlog).
  const anchored = defaultDate ? defaultDate.slice(0, 10) : null;
  // Same fallback chain as the event picker: explicit default →
  // last-used (if still present and writable) → first writable →
  // first list at all.
  const writableLists = taskLists.filter((l) => !l.read_only);
  const lastUsed = readLastUsedTaskList();
  const lastUsedIfValid =
    lastUsed && writableLists.some((l) => l.id === lastUsed)
      ? lastUsed
      : null;
  const fallbackList =
    defaultListId ??
    lastUsedIfValid ??
    writableLists[0]?.id ??
    taskLists[0]?.id ??
    '';
  return {
    title: '',
    listId: fallbackList,
    status: 'open',
    priority: 'medium',
    deadlineMode: anchored ? 'scheduled' : 'none',
    date: anchored ?? todayInput(),
    time: '09:00',
    description: '',
    colorLabel: null,
    recurrence: { ...TASK_RECURRENCE_DEFAULT },
    reminders: [],
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
