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
  isCommandError,
  updateEvent as apiUpdateEvent,
} from '../api/client';
import { invoke } from '@tauri-apps/api/core';
import type { CalendarEvent, Task } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';
import { useCalendarStore } from '../state/CalendarStore';
import type { MoveCopyTarget } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
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

  // Callers (esp. the chip context menu's "Kopieren nach …" entry)
  // can pre-select either mode. Default stays `move` to match the
  // long-standing Shift+M behaviour.
  const [mode, setMode] = useState<Mode>(target.defaultMode ?? 'move');
  const [targetContainerId, setTargetContainerId] = useState(initialContainerId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // Subtasks travel with their parent — no opt-out. A parent in
  // list A with a subtask left behind in list B would split a
  // logical unit across two containers, which the rest of the app
  // (TaskDialog, TaskView, calendar filter) treats as impossible.
  // The dialog still surfaces a one-line info when children exist
  // so the user knows the move/copy is going to touch more rows
  // than just the focused one.
  const { tasks } = useTasks();
  const children = useMemo(() => {
    if (target.kind !== 'task') return [];
    return tasks.filter((row) => row.parent_id === target.task.id);
  }, [tasks, target]);

  const containers = useMemo(() => {
    // Move / copy targets must accept writes — drop read-only
    // containers (iCal feeds, future shared read-only sources) so
    // the picker never offers a destination the backend would
    // reject with "Unsupported".
    if (target.kind === 'event') {
      return calendars
        .filter((c) => !c.read_only)
        .map((c) => ({ id: c.id, name: c.name }));
    }
    return taskLists
      .filter((l) => !l.read_only)
      .map((l) => ({ id: l.id, name: l.name }));
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
          await moveOrCopyTask(
            target.task,
            targetContainerId,
            mode,
            children,
          );
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
      children,
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

        {target.kind === 'task' && children.length > 0 && (
          <p className="form__hint">
            {/* Subtasks always travel with their parent; the
                checkbox went away once we settled the invariant
                that a task and its children must live in the same
                list. The line still surfaces so the user isn't
                surprised by an N-row mutation. */}
            {t('dialogs.moveCopy.subtasksIncluded', {
              count: children.length,
            })}
          </p>
        )}

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <div className="form__actions">
          <button
            type="button"
            onClick={onClose}
            aria-disabled={submitting || undefined}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            aria-disabled={submitting || undefined}
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
  // For expanded occurrences the underlying row is the series; we
  // never move just one instance of a recurring series. `seriesIdOf`
  // pulls the master id off the augmented event shape (and handles
  // CalDAV UIDs that themselves contain '@' — see the helper for
  // why a `split('@')` shortcut would corrupt them).
  const seriesId = seriesIdOf(event);

  if (mode === 'move') {
    // Pass the original calendar_id as the move hint. The backend
    // uses it to detect cross-calendar moves: local-only stays a
    // single SQL UPDATE on the calendar_id column; for external
    // adapters (iCloud, Google, Graph, EWS) the backend reroutes
    // as create-on-target + delete-from-source — without this
    // hint, a CalDAV move would PUT to a non-existent resource on
    // the new calendar with the old etag's If-Match, drawing a
    // 412 from iCloud.
    await apiUpdateEvent(
      {
        ...event,
        id: seriesId,
        calendar_id: targetCalendarId,
      },
      event.calendar_id,
    );
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
  children: Task[],
): Promise<void> {
  if (mode === 'move') {
    // Pass the original list_id as the move hint. Local-only stays
    // a single SQL UPDATE (row identity preserved); cross-adapter
    // moves go through create-on-target + delete-from-source on the
    // backend, which gives the task a new id.
    //
    // Children inherit the same move: each one's previous list_id
    // is wherever it currently lives (`child.list_id`), the target
    // is the new list. For *cross-adapter* moves the parent's id
    // changes, so we re-thread each child's parent_id onto the
    // freshly created parent id the backend just returned. For
    // local moves the id stays the same and this is a no-op
    // rewrite. Either way the family stays connected.
    const movedParent = await invoke<Task>('update_task', {
      task: { ...task, list_id: targetListId },
      previousListId: task.list_id,
    });
    for (const child of children) {
      await invoke<Task>('update_task', {
        task: {
          ...child,
          list_id: targetListId,
          parent_id: movedParent.id,
        },
        previousListId: child.list_id,
      });
    }
    return;
  }

  // Copy: create a fresh parent row, then re-parent each child copy
  // onto the new id. The original parent and its children stay put.
  const newParent = await apiCreateTask({
    list_id: targetListId,
    title: task.title,
    description: task.description,
    status: task.status,
    priority: task.priority,
    scheduled_date: task.scheduled_date,
    scheduled_time: task.scheduled_time,
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
    recurrence: task.recurrence,
    parent_id: null,
    // Copy lands in another list whose sections differ — start ungrouped.
    section_id: null,
    color_label: task.color_label,
    reminders: task.reminders,
    sound: task.sound,
  });
  for (const child of children) {
    await apiCreateTask({
      list_id: targetListId,
      title: child.title,
      description: child.description,
      status: child.status,
      priority: child.priority,
      scheduled_date: child.scheduled_date,
      scheduled_time: child.scheduled_time,
      deadline_date: child.deadline_date,
      deadline_time: child.deadline_time,
      recurrence: child.recurrence,
      parent_id: newParent.id,
      section_id: null,
      color_label: child.color_label,
      reminders: child.reminders,
      sound: child.sound,
    });
  }
}

