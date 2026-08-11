import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { selectableEventCalendars, selectableTaskLists } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  createSection as apiCreateSection,
  createTask as apiCreateTask,
  isCommandError,
} from '../api/client';
import type { Task } from '../api/types';
import { isSeriesOccurrence } from '../intl/recurrence';
import {
  moveOrCopyEvent,
  moveTaskToList,
  type MoveCopyScope,
} from '../state/moveActions';
import { useCalendarStore } from '../state/calendarStoreContext';
import { canAssignSection } from '../state/taskMoves';
import type { MoveCopyTarget } from '../state/DialogState';
import { useTasks } from '../state/useTasks';
import { useViewState } from '../state/viewStateContext';
import { Modal } from './Modal';

/**
 * Move / copy a single event or task to a different container.
 *
 * The dialog handles both kinds with one shape:
 *  - For events: pick a target calendar.
 *  - For tasks: pick a target list — and, where the list has them, the
 *    SECTION inside it, which can be created here rather than in a detour
 *    through the editor. Filing something is one thought; it should not need
 *    two dialogs.
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
  const {
    calendars,
    taskLists,
    selectedCalendarIds,
    selectedTaskListIds,
    sectionsByList,
    loadSections,
  } = useCalendarStore();
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
  /** The section inside the target list; `''` is "no section". */
  const [targetSectionId, setTargetSectionId] = useState('');
  /** Non-null while the inline "new section" name box is open. */
  const [sectionDraft, setSectionDraft] = useState<string | null>(null);
  const [sectionBusy, setSectionBusy] = useState(false);
  const sectionSelectRef = useRef<HTMLSelectElement>(null);
  const sectionFieldId = useId();
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

  // Sections belong to the TARGET list, not the source: what the user is
  // choosing is where the task lands.
  const targetList = useMemo(
    () => taskLists.find((l) => l.id === targetContainerId),
    [taskLists, targetContainerId],
  );
  const sectionsEnabled = target.kind === 'task' && canAssignSection(targetList);
  // Creating one needs the adapter to say it can (local lists, Todoist,
  // Vikunja). A provider that only exposes sections read-only still gets the
  // picker — it just cannot grow a new one from here.
  const sectionsManageable =
    sectionsEnabled && !!targetList?.task_capabilities?.manageable_sections;
  const sectionsForTarget = useMemo(
    () => sectionsByList[targetContainerId] ?? [],
    [sectionsByList, targetContainerId],
  );

  useEffect(() => {
    if (!isOpen || !sectionsEnabled) return;
    if (targetContainerId in sectionsByList) return;
    void loadSections(targetContainerId);
  }, [isOpen, sectionsEnabled, targetContainerId, sectionsByList, loadSections]);

  // A section id only means something inside its own list, so switching the
  // target drops the choice rather than carrying a stale one across.
  useEffect(() => {
    setTargetSectionId('');
    setSectionDraft(null);
  }, [targetContainerId]);

  /**
   * Make a section in the target list and select it.
   *
   * Immediate, like the editor's: the section is a real row the moment it is
   * named, and the move that follows files into it. Position 0 puts a section
   * made in passing at the top, where the thing being filed is about to be.
   */
  const createTargetSection = useCallback(async () => {
    const name = (sectionDraft ?? '').trim();
    if (!name) {
      setSectionDraft(null);
      sectionSelectRef.current?.focus({ preventScroll: true });
      return;
    }
    setSectionBusy(true);
    try {
      const created = await apiCreateSection({
        list_id: targetContainerId,
        name,
        position: 0,
      });
      await loadSections(targetContainerId);
      setTargetSectionId(created.id);
      announce(t('dialogs.task.section.created', { name: created.name }));
      setSectionDraft(null);
    } catch (err) {
      setError(isCommandError(err) ? `${err.code}: ${err.message}` : String(err));
    } finally {
      setSectionBusy(false);
      // Back to the picker, which now holds the new section — the user asked
      // where this lands, and that is the control that answers.
      sectionSelectRef.current?.focus({ preventScroll: true });
    }
  }, [sectionDraft, targetContainerId, loadSections, announce, t]);

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
            sectionsEnabled ? targetSectionId || null : null,
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
      sectionsEnabled,
      targetSectionId,
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

        {sectionsEnabled && (
          <div className="form__field">
            <label className="form__label" htmlFor={sectionFieldId}>
              {t('dialogs.task.fields.section')}
            </label>
            <div className="section-field">
              <select
                id={sectionFieldId}
                ref={sectionSelectRef}
                value={targetSectionId}
                onChange={(e) => setTargetSectionId(e.target.value)}
                disabled={sectionDraft !== null}
              >
                <option value="">{t('dialogs.task.noSection')}</option>
                {sectionsForTarget.map((section) => (
                  <option key={section.id} value={section.id}>
                    {section.name}
                  </option>
                ))}
              </select>
              {sectionsManageable && sectionDraft === null && (
                <button
                  type="button"
                  className="section-field__button"
                  onClick={() => setSectionDraft('')}
                >
                  {t('dialogs.task.section.add')}
                </button>
              )}
            </div>
            {/* Naming it here rather than in a second dialog: filing
                something into a section that does not exist yet is one
                thought, and the detour through the editor was the reason
                people left things unfiled. */}
            {sectionDraft !== null && (
              <div className="section-field__edit">
                <input
                  type="text"
                  value={sectionDraft}
                  autoFocus
                  aria-label={t('dialogs.task.section.nameField')}
                  placeholder={t('dialogs.task.section.namePlaceholder')}
                  onChange={(e) => setSectionDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      // Enter must not submit the surrounding form — that
                      // would move the task before its section exists.
                      e.preventDefault();
                      void createTargetSection();
                    } else if (e.key === 'Escape') {
                      e.preventDefault();
                      e.stopPropagation();
                      setSectionDraft(null);
                      sectionSelectRef.current?.focus({ preventScroll: true });
                    }
                  }}
                />
                <button
                  type="button"
                  className="section-field__button"
                  aria-disabled={sectionBusy || undefined}
                  onClick={() => void createTargetSection()}
                >
                  {t('dialogs.task.section.create')}
                </button>
                <button
                  type="button"
                  className="section-field__button"
                  onClick={() => {
                    setSectionDraft(null);
                    sectionSelectRef.current?.focus({ preventScroll: true });
                  }}
                >
                  {t('dialogs.task.section.cancel')}
                </button>
              </div>
            )}
          </div>
        )}

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
  sectionId: string | null,
): Promise<void> {
  if (mode === 'move') {
    // Reuses the shared move primitive (see moveActions.ts for the
    // move-hint + child-re-threading rationale).
    await moveTaskToList(task, targetListId, children, sectionId);
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
    // The section the user picked in the target list, or none. It used to be
    // unconditionally null, with "another list's sections differ" as the
    // reason — true, and the reason to ASK rather than to decide.
    section_id: sectionId,
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
      // Children file where their parent does; a family split across
      // sections is the same surprise as one split across lists.
      section_id: sectionId,
      color_label: child.color_label,
      reminders: child.reminders,
      assignees: [],
      sound: child.sound,
    });
  }
}

