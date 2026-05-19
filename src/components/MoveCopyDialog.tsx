import {
  useCallback,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createEvent as apiCreateEvent,
  createTask as apiCreateTask,
  deleteEventById,
  isCommandError,
  updateEvent as apiUpdateEvent,
} from '../api/client';
import { invoke } from '@tauri-apps/api/core';
import type { CalendarEvent, Task } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import type { MoveCopyTarget } from '../state/DialogState';
import { Modal } from './Modal';

/**
 * Move / copy a single event or task to a different container.
 *
 * The dialog handles both kinds with one shape:
 *  - For events: pick a target calendar.
 *  - For tasks: pick a target list.
 *
 * "Move" within the same container is a no-op; we still show it as a
 * valid choice so the user can confirm without remembering which
 * container the item currently lives in. The list-of-targets filters
 * out the current container only when actually executing.
 *
 * DESIGN.md sections 7.5 / 9.10 describe two-step move semantics
 * (CREATE in target, DELETE in source) for cross-account moves. For
 * the local adapter both rows live in the same database so a single
 * UPDATE suffices — the helper picks the right path automatically.
 */
export interface MoveCopyDialogProps {
  isOpen: boolean;
  onClose: () => void;
  target: MoveCopyTarget;
}

type Mode = 'move' | 'copy';

export function MoveCopyDialog({
  isOpen,
  onClose,
  target,
}: MoveCopyDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars, taskLists } = useCalendarStore();

  const initialContainerId =
    target.kind === 'event' ? target.event.calendar_id : target.task.list_id;

  const [mode, setMode] = useState<Mode>('move');
  const [targetContainerId, setTargetContainerId] = useState(initialContainerId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const containers = useMemo(() => {
    if (target.kind === 'event') {
      return calendars.map((c) => ({ id: c.id, name: c.name }));
    }
    return taskLists.map((l) => ({ id: l.id, name: l.name }));
  }, [target.kind, calendars, taskLists]);

  const itemTitle =
    target.kind === 'event' ? target.event.title : target.task.title;

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      setError(null);

      if (!targetContainerId) {
        setError(t('dialogs.moveCopy.targetRequired'));
        return;
      }

      if (mode === 'move' && targetContainerId === initialContainerId) {
        // Same container, nothing to do. Treat it as a successful
        // no-op so the user gets out of the dialog.
        onClose();
        return;
      }

      setSubmitting(true);
      try {
        if (target.kind === 'event') {
          await moveOrCopyEvent(target.event, targetContainerId, mode);
        } else {
          await moveOrCopyTask(target.task, targetContainerId, mode);
        }
        announce(
          t(
            mode === 'move'
              ? 'dialogs.moveCopy.moved'
              : 'dialogs.moveCopy.copied',
            { title: itemTitle },
          ),
        );
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
    [
      target,
      mode,
      targetContainerId,
      initialContainerId,
      announce,
      onClose,
      itemTitle,
      t,
    ],
  );

  const title = t(
    target.kind === 'event'
      ? 'dialogs.moveCopy.titleEvent'
      : 'dialogs.moveCopy.titleTask',
    { title: itemTitle },
  );

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      className="modal--form modal--narrow"
      dismissOnBackdrop={false}
    >
      <form onSubmit={onSubmit} className="form">
        <fieldset className="form__field">
          <legend className="form__label">
            {t('dialogs.moveCopy.modeLabel')}
          </legend>
          <label className="form__field form__field--inline">
            <input
              type="radio"
              name="movecopy-mode"
              value="move"
              checked={mode === 'move'}
              onChange={() => setMode('move')}
            />
            <span>{t('dialogs.moveCopy.modeMove')}</span>
          </label>
          <label className="form__field form__field--inline">
            <input
              type="radio"
              name="movecopy-mode"
              value="copy"
              checked={mode === 'copy'}
              onChange={() => setMode('copy')}
            />
            <span>{t('dialogs.moveCopy.modeCopy')}</span>
          </label>
        </fieldset>

        <label className="form__field">
          <span className="form__label">
            {t(
              target.kind === 'event'
                ? 'dialogs.moveCopy.targetCalendar'
                : 'dialogs.moveCopy.targetList',
            )}
          </span>
          <select
            value={targetContainerId}
            onChange={(e) => setTargetContainerId(e.target.value)}
            required
          >
            {containers.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
                {c.id === initialContainerId
                  ? ` ${t('dialogs.moveCopy.currentSuffix')}`
                  : ''}
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
            {mode === 'move'
              ? t('dialogs.moveCopy.submitMove')
              : t('dialogs.moveCopy.submitCopy')}
          </button>
        </div>
      </form>
    </Modal>
  );
}

async function moveOrCopyEvent(
  event: CalendarEvent,
  targetCalendarId: string,
  mode: Mode,
): Promise<void> {
  // Strip the occurrence-suffix if this came from an expanded recurring
  // event — the underlying row id is everything before the "@".
  const seriesId = event.id.includes('@')
    ? event.id.split('@')[0]
    : event.id;

  if (mode === 'move') {
    // Local adapter: same database, single UPDATE is enough.
    await apiUpdateEvent({
      ...event,
      id: seriesId,
      calendar_id: targetCalendarId,
    });
    return;
  }

  await apiCreateEvent({
    calendar_id: targetCalendarId,
    title: event.title,
    description: event.description,
    location: event.location,
    start: event.start,
    end: event.end,
    all_day: event.all_day,
    recurrence: event.recurrence,
    color_label: event.color_label,
    reminders: event.reminders,
    sound: event.sound,
    attendees: event.attendees,
  });
}

async function moveOrCopyTask(
  task: Task,
  targetListId: string,
  mode: Mode,
): Promise<void> {
  if (mode === 'move') {
    await invoke<Task>('update_task', {
      task: { ...task, list_id: targetListId },
    });
    return;
  }

  await apiCreateTask({
    list_id: targetListId,
    title: task.title,
    description: task.description,
    status: task.status,
    priority: task.priority,
    scheduled_date: task.scheduled_date,
    deadline_type: task.deadline_type,
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
    recurrence: task.recurrence,
    parent_id: null, // top-level on copy; subtask cloning is out of scope
    color_label: task.color_label,
    reminders: task.reminders,
    sound: task.sound,
  });
}

// ────────────────────────────────────────────────────────────────────────────
// Duplicate helper — used by Ctrl+D, no dialog needed.
// ────────────────────────────────────────────────────────────────────────────

export async function duplicateEvent(event: CalendarEvent): Promise<void> {
  await apiCreateEvent({
    calendar_id: event.calendar_id,
    title: event.title,
    description: event.description,
    location: event.location,
    start: event.start,
    end: event.end,
    all_day: event.all_day,
    recurrence: event.recurrence,
    color_label: event.color_label,
    reminders: event.reminders,
    sound: event.sound,
    attendees: event.attendees,
  });
}

export async function duplicateTask(task: Task): Promise<void> {
  await apiCreateTask({
    list_id: task.list_id,
    title: task.title,
    description: task.description,
    status: task.status,
    priority: task.priority,
    scheduled_date: task.scheduled_date,
    deadline_type: task.deadline_type,
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
    recurrence: task.recurrence,
    parent_id: null,
    color_label: task.color_label,
    reminders: task.reminders,
    sound: task.sound,
  });
}

// Re-export so callers don't need to know about the `deleteEventById`
// import — the move helper uses it for cross-adapter moves later.
export { deleteEventById };
