import {
  useCallback,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { selectableEventCalendars, selectableTaskLists } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { createTask as apiCreateTask, isCommandError } from '../api/client';
import type { Task } from '../api/types';
import { isSeriesOccurrence } from '../intl/recurrence';
import {
  moveOrCopyEvent,
  moveTaskToList,
  type MoveCopyScope,
} from '../state/moveActions';
import { useCalendarStore } from '../state/calendarStoreContext';
import type { MoveCopyTarget } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { useViewState } from '../state/viewStateContext';
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
  const { calendars, taskLists, selectedCalendarIds, selectedTaskListIds } =
    useCalendarStore();
  const { showHiddenCalendarTargets, showHiddenTaskListTargets } =
    useViewState();

  const initialContainerId =
    target.kind === 'event' ? target.event.calendar_id : target.task.list_id;

  // Callers (esp. the chip context menu's "Kopieren nach …" entry)
  // can pre-select either mode. Default stays `move` to match the
  // long-standing Shift+M behaviour.
  const [mode, setMode] = useState<Mode>(target.defaultMode ?? 'move');
  // Recurring events reach the dialog as expanded occurrences; only then is
  // the "this occurrence vs the whole series" choice meaningful (§7.5).
  const isRecurringOccurrence =
    target.kind === 'event' && isSeriesOccurrence(target.event);
  const [scope, setScope] = useState<MoveCopyScope>('occurrence');
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
    // reject with "Unsupported". Also drop containers the sidebar
    // hides / unchecks, matching the editors' target pickers.
    if (target.kind === 'event') {
      return selectableEventCalendars(calendars, {
        selectedIds: selectedCalendarIds,
        currentId: initialContainerId,
        includeHidden: showHiddenCalendarTargets,
      }).map((c) => ({ id: c.id, name: c.name }));
    }
    return selectableTaskLists(taskLists, {
      selectedIds: selectedTaskListIds,
      currentId: initialContainerId,
      includeHidden: showHiddenTaskListTargets,
    }).map((l) => ({ id: l.id, name: l.name }));
  }, [
    target.kind,
    calendars,
    taskLists,
    selectedCalendarIds,
    selectedTaskListIds,
    initialContainerId,
    showHiddenCalendarTargets,
    showHiddenTaskListTargets,
  ]);

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
          await moveOrCopyEvent(target.event, targetContainerId, mode, scope);
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
      scope,
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

        {isRecurringOccurrence && (
          <fieldset className="form__field">
            <legend className="form__label">
              {t('dialogs.moveCopy.scopeLabel')}
            </legend>
            <label className="form__field form__field--inline">
              <input
                type="radio"
                name="movecopy-scope"
                value="occurrence"
                checked={scope === 'occurrence'}
                onChange={() => setScope('occurrence')}
              />
              <span>{t('dialogs.moveCopy.scopeOccurrence')}</span>
            </label>
            <label className="form__field form__field--inline">
              <input
                type="radio"
                name="movecopy-scope"
                value="series"
                checked={scope === 'series'}
                onChange={() => setScope('series')}
              />
              <span>{t('dialogs.moveCopy.scopeSeries')}</span>
            </label>
          </fieldset>
        )}

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

async function moveOrCopyTask(
  task: Task,
  targetListId: string,
  mode: Mode,
  children: Task[],
): Promise<void> {
  if (mode === 'move') {
    // Reuses the shared move primitive (see moveActions.ts for the
    // move-hint + child-re-threading rationale).
    await moveTaskToList(task, targetListId, children);
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
    effort: task.effort,
    scheduled_date: task.scheduled_date,
    scheduled_time: task.scheduled_time,
    deadline_date: task.deadline_date,
    deadline_time: task.deadline_time,
    deadline_reminder_days: task.deadline_reminder_days,
    recurrence: task.recurrence,
    parent_id: null,
    // Copy lands in another list whose sections differ — start ungrouped.
    section_id: null,
    color_label: task.color_label,
    reminders: task.reminders,
    // Copy lands in another list whose members differ — start unassigned.
    assignees: [],
    sound: task.sound,
  });
  for (const child of children) {
    await apiCreateTask({
      list_id: targetListId,
      title: child.title,
      description: child.description,
      status: child.status,
      priority: child.priority,
      effort: child.effort,
      scheduled_date: child.scheduled_date,
      scheduled_time: child.scheduled_time,
      deadline_date: child.deadline_date,
      deadline_time: child.deadline_time,
      deadline_reminder_days: child.deadline_reminder_days,
      recurrence: child.recurrence,
      parent_id: newParent.id,
      section_id: null,
      color_label: child.color_label,
      reminders: child.reminders,
      assignees: [],
      sound: child.sound,
    });
  }
}

