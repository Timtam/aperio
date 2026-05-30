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

import { DescriptionLinks } from './DescriptionLinks';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createSection,
  createTask as apiCreateTask,
  deleteSection,
  isCommandError,
  showContextMenu,
  updateSection,
  type ContextMenuItemRequest,
} from '../api/client';
import { invoke } from '@tauri-apps/api/core';
import { todayIsoKey } from '../intl/taskDay';
import { statusI18nKey, statusMarker } from '../intl/taskStatus';
import type { Reminder, Task, TaskPriority, TaskStatus } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { canAssignSection, canMoveTaskBetweenLists } from '../state/taskMoves';
import { useDialogState } from '../state/DialogState';
import {
  planAncestorRecompute,
  planStatusCascade,
  type StatusWrite,
} from '../state/taskCascade';
import { useTaskCascadeEnabled } from '../state/TaskCascadeProvider';
import { useTasks } from '../state/useTasks';
import { useTaskStatusActions } from '../state/useTaskStatusToggle';
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


interface FormState {
  title: string;
  listId: string;
  /** Section id within `listId`, or '' for ungrouped. Only meaningful
   *  when the list's adapter declares the `sections` capability. */
  sectionId: string;
  status: TaskStatus;
  priority: TaskPriority;
  // Two independent date+time slots — replacing the old `deadlineMode`
  // + single date/time fields. Each pair maps directly onto the
  // matching wire fields (`scheduled_*` / `deadline_*`); an empty
  // date string means the slot is unset. Time without date is
  // impossible in the UI (the time input is disabled when the date
  // is empty) and would also fail the DB CHECK constraint.
  scheduledDate: string;
  scheduledTime: string;
  deadlineDate: string;
  deadlineTime: string;
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
  const { taskLists, colorLabels, sectionsByList, loadSections } =
    useCalendarStore();
  const { tasks } = useTasks();
  const { invalidateData } = useDialogState();
  // Shared status actions — they own the parent/subtask cascade
  // (taskCascade.ts) and the SR announce. Used both for the listbox
  // Space-toggle and the per-row context menu.
  const { toggle: toggleSubtaskAction, set: setSubtaskAction } =
    useTaskStatusActions();
  // Settings → Tasks → "couple parent and subtask status" + auto-date
  // toggle. The first short-circuits both planners; the second drops
  // the `todayKey` companion-write so a started backlog task isn't
  // auto-pinned to today.
  const { enabled: cascadeEnabled, autoDate } = useTaskCascadeEnabled();

  const isEdit = task !== null;
  // Subtask = a task that has a parent. The list dropdown locks
  // for these; their list always tracks their parent's. The flag
  // also drives the cascade decision on save below.
  const isSubtask = task !== null && task.parent_id !== null;
  const subtaskHintId = useId();
  const moveLockHintId = useId();
  const sectionFieldId = useId();

  // Subtasks: children of the task currently being edited. Only
  // meaningful in edit mode — a brand-new task has no id yet, so
  // children can't reference it. Filtering here over the global
  // task list keeps the dialog in sync with TaskView whenever the
  // user (or a sync) mutates a child elsewhere.
  const subtasks = useMemo<Task[]>(() => {
    if (!task) return [];
    return tasks.filter((row) => row.parent_id === task.id);
  }, [tasks, task]);

  // Does the edited task's list support subtasks? Gates the
  // add-subtask affordance. Absent capabilities default to the
  // cal-core-native "subtasks: true", so local + simple backends keep
  // the feature; an adapter that declares `subtasks: false` (e.g. EWS)
  // hides it. Existing children stay visible regardless.
  const supportsSubtasks = useMemo(() => {
    if (!task) return true;
    const list = taskLists.find((l) => l.id === task.list_id);
    return list?.task_capabilities?.subtasks ?? true;
  }, [task, taskLists]);

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

  // Section assignment (TASKS-11): the list the form currently targets
  // drives which sections are offered and whether the picker shows at
  // all. Cross-list moves are gated on the *source* list's
  // move_between_projects (Todoist locks it).
  const listForSections = taskLists.find((l) => l.id === form.listId);
  const sourceList = task
    ? taskLists.find((l) => l.id === task.list_id)
    : undefined;
  const sectionsEnabled = canAssignSection(listForSections);
  const canMoveLists = canMoveTaskBetweenLists(sourceList);
  // Lock the list picker when editing a task whose source adapter
  // can't reparent tasks (Todoist). Creation is unaffected — you can
  // always pick the list for a brand-new task.
  const moveLocked = isEdit && !canMoveLists;
  const sectionsForList = sectionsByList[form.listId] ?? [];
  // Sections are user-managed (create / rename / delete) only on local
  // lists; external-provider sections are read-only here (managed in
  // the provider's own UI), so we just offer the picker for those.
  const sectionsManageable =
    sectionsEnabled && listForSections?.account_id === 'local';

  // Pull the target list's sections when the picker is relevant.
  useEffect(() => {
    if (isOpen && sectionsEnabled && !(form.listId in sectionsByList)) {
      void loadSections(form.listId);
    }
  }, [isOpen, sectionsEnabled, form.listId, sectionsByList, loadSections]);

  // If the chosen section no longer belongs to the selected list (the
  // user switched lists), drop it back to ungrouped.
  useEffect(() => {
    if (
      form.sectionId &&
      form.listId in sectionsByList &&
      !sectionsForList.some((s) => s.id === form.sectionId)
    ) {
      setForm((prev) => ({ ...prev, sectionId: '' }));
    }
  }, [form.sectionId, form.listId, sectionsByList, sectionsForList]);

  // ── Section management (local lists only) ───────────────────────────
  // `sectionMode` reveals an inline name input for create / rename; the
  // commands persist immediately (like subtasks) and reload the list's
  // sections so the picker reflects the change.
  const [sectionMode, setSectionMode] = useState<'create' | 'rename' | null>(
    null,
  );
  const [sectionDraft, setSectionDraft] = useState('');
  const [sectionBusy, setSectionBusy] = useState(false);

  // Reset the inline editor whenever the dialog re-opens or the target
  // list changes (its sections — and thus what's manageable — differ).
  useEffect(() => {
    setSectionMode(null);
    setSectionDraft('');
  }, [isOpen, form.listId]);

  const submitSection = useCallback(async () => {
    const name = sectionDraft.trim();
    if (!name || sectionBusy) return;
    setSectionBusy(true);
    try {
      if (sectionMode === 'rename' && form.sectionId) {
        const current = sectionsForList.find((s) => s.id === form.sectionId);
        if (current) {
          await updateSection({ ...current, name });
          announce(t('dialogs.task.section.renamed', { name }));
        }
      } else {
        const created = await createSection({
          list_id: form.listId,
          name,
          position: sectionsForList.length,
        });
        // Reload BEFORE selecting the new section so the
        // "section gone from list" reset effect sees it as valid and
        // doesn't race-clear the fresh selection.
        await loadSections(form.listId);
        setForm((prev) => ({ ...prev, sectionId: created.id }));
        announce(t('dialogs.task.section.created', { name }));
        setSectionMode(null);
        setSectionDraft('');
        return;
      }
      await loadSections(form.listId);
      setSectionMode(null);
      setSectionDraft('');
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('section mutation failed', err);
    } finally {
      setSectionBusy(false);
    }
  }, [
    sectionDraft,
    sectionBusy,
    sectionMode,
    form.sectionId,
    form.listId,
    sectionsForList,
    loadSections,
    announce,
    t,
  ]);

  const deleteCurrentSection = useCallback(async () => {
    if (!form.sectionId || sectionBusy) return;
    const current = sectionsForList.find((s) => s.id === form.sectionId);
    setSectionBusy(true);
    try {
      // Deleting a section only ungroups its tasks (ON DELETE SET NULL),
      // so there's no destructive-confirm — the tasks survive.
      await deleteSection(form.sectionId);
      setForm((prev) => ({ ...prev, sectionId: '' }));
      await loadSections(form.listId);
      announce(
        t('dialogs.task.section.deleted', { name: current?.name ?? '' }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('delete_section failed', err);
    } finally {
      setSectionBusy(false);
    }
  }, [
    form.sectionId,
    form.listId,
    sectionBusy,
    sectionsForList,
    loadSections,
    announce,
    t,
  ]);

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
      const created = await apiCreateTask({
        list_id: task.list_id,
        title: trimmed,
        description: null,
        status: 'open',
        priority: 'medium',
        scheduled_date: null,
        scheduled_time: null,
        deadline_date: null,
        deadline_time: null,
        recurrence: null,
        parent_id: task.id,
        // Keep the subtask in its parent's section so it groups with it.
        section_id: task.section_id,
        color_label: null,
        reminders: [],
        sound: null,
      });
      // Cascade-up: if the parent was completed before this new
      // open child appeared, the parent should re-derive to
      // in_progress (or open if no other progress exists). The
      // snapshot we hand the planner has to *include* the freshly
      // created row, since `tasks` is the pre-mutation cache.
      await applyAncestorWrites(
        planAncestorRecompute(task.id, [...tasks, created], {
          cascadeEnabled,
          ...(autoDate ? { todayKey: todayIsoKey() } : {}),
        }),
        [...tasks, created],
      );
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
  }, [
    task,
    newSubtaskTitle,
    subtaskBusy,
    invalidateData,
    announce,
    t,
    tasks,
    cascadeEnabled,
    autoDate,
  ]);

  // Subtask status changes go through the shared hook — that gives
  // us the same cascade (recompute parent, propagate further up) we
  // get on every other task surface.
  const toggleSubtaskStatus = useCallback(
    async (subtask: Task) => {
      await toggleSubtaskAction(subtask);
    },
    [toggleSubtaskAction],
  );

  const deleteSubtask = useCallback(
    async (subtask: Task) => {
      try {
        await invoke<void>('delete_task', {
          id: subtask.id,
          listId: subtask.list_id,
        });
        // Recompute ancestors against the post-deletion snapshot:
        // removing the last open child should mark the parent
        // completed (or whatever its other children imply).
        const parentId = subtask.parent_id;
        if (parentId) {
          const snapshot = tasks.filter((row) => row.id !== subtask.id);
          await applyAncestorWrites(
            planAncestorRecompute(parentId, snapshot, {
              cascadeEnabled,
              ...(autoDate ? { todayKey: todayIsoKey() } : {}),
            }),
            snapshot,
          );
        }
        invalidateData();
        announce(t('dialogs.task.deleted', { title: subtask.title }));
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [invalidateData, announce, t, tasks, cascadeEnabled, autoDate],
  );

  const setSubtaskStatus = useCallback(
    async (subtask: Task, next: TaskStatus) => {
      await setSubtaskAction(subtask, next);
    },
    [setSubtaskAction],
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

      const {
        scheduled_date,
        scheduled_time,
        deadline_date,
        deadline_time,
      } = splitDeadline(form);

      setSubmitting(true);
      try {
        if (isEdit && task) {
          const updated: Task = {
            ...task,
            title: trimmedTitle,
            list_id: form.listId,
            // A cross-list move can't keep the old list's section, so
            // clear it; an in-place edit takes the picker's value.
            section_id:
              form.listId !== task.list_id ? null : form.sectionId || null,
            status: form.status,
            priority: form.priority,
            scheduled_date,
            scheduled_time,
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
          // Pass the *original* list_id as the move hint so the
          // backend can tell an in-place edit (list picker
          // untouched) apart from a cross-list move (picker
          // pointing somewhere new). Without it, a CalDAV-VTODO
          // edit that changes the list would PATCH the wrong
          // resource URL and iCloud-shaped servers reject the
          // precondition with 412 / Conflict — same root cause
          // the event-side `previousCalendarId` hint addresses.
          await invoke<Task>('update_task', {
            task: updated,
            previousListId: task.list_id,
          });
          // List-cohabitation: if a parent's list changed, every
          // descendant follows along so the family stays together.
          // Otherwise a subsequent reload would surface a parent in
          // list B with children stranded in list A — exactly the
          // split-state the move/copy invariant prevents elsewhere.
          if (
            !isSubtask &&
            form.listId !== task.list_id
          ) {
            const descendants = collectDescendants(task.id, tasks);
            for (const child of descendants) {
              // Each descendant's *previous* list is wherever it
              // already lives (task.list_id, captured before the
              // parent's move); the post-move target is the new
              // form.listId.
              await invoke<Task>('update_task', {
                task: { ...child, list_id: form.listId },
                previousListId: child.list_id,
              });
            }
          }
          // Status cascade: if the user changed the Status dropdown
          // on the form, propagate the change through the family
          // (cascade-down for completed/cancelled; cascade-up for
          // every status). Skip the root write — we just did it
          // above with the full field set. The snapshot we hand the
          // planner reflects the *new* status so up-cascade reads
          // a coherent state.
          if (form.status !== task.status) {
            const snapshot = tasks.map((row) =>
              row.id === task.id ? { ...row, status: form.status } : row,
            );
            const cascadeWrites = planStatusCascade(
              task.id,
              form.status,
              snapshot,
              {
                cascadeEnabled,
                ...(autoDate ? { todayKey: todayIsoKey() } : {}),
              },
            ).filter((w) => w.taskId !== task.id);
            await applyAncestorWrites(cascadeWrites, snapshot);
          }
          announce(t('dialogs.task.updated', { title: trimmedTitle }));
        } else {
          await apiCreateTask({
            list_id: form.listId,
            title: trimmedTitle,
            description: form.description.trim() || null,
            status: form.status,
            priority: form.priority,
            scheduled_date,
            scheduled_time,
            deadline_date,
            deadline_time,
            recurrence: recurrenceToBackend(form.recurrence),
            parent_id: null,
            section_id: form.sectionId || null,
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
    [
      form,
      submitting,
      isEdit,
      task,
      announce,
      onClose,
      t,
      isSubtask,
      tasks,
      cascadeEnabled,
      autoDate,
    ],
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
            // Subtasks must live in the same list as their parent
            // — moving a subtask alone would split a logical
            // unit. The "move/copy with parent" path is the only
            // way to relocate a subtask; the disabled dropdown is
            // accompanied by a hint below so the rule is visible.
            // The picker is also locked when editing a task whose
            // source adapter can't move tasks between projects
            // (Todoist) — see canMoveTaskBetweenLists.
            disabled={isSubtask || moveLocked}
            aria-describedby={
              isSubtask
                ? subtaskHintId
                : moveLocked
                  ? moveLockHintId
                  : undefined
            }
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
          {isSubtask && (
            <p id={subtaskHintId} className="form__hint">
              {t('dialogs.task.subtaskListLocked')}
            </p>
          )}
          {!isSubtask && moveLocked && (
            <p id={moveLockHintId} className="form__hint">
              {t('dialogs.task.listMoveLocked')}
            </p>
          )}
        </label>

        {sectionsEnabled && (
          <div className="form__field">
            <label className="form__label" htmlFor={sectionFieldId}>
              {t('dialogs.task.fields.section')}
            </label>
            <div className="section-field">
              <select
                id={sectionFieldId}
                value={form.sectionId}
                onChange={(e) => update('sectionId', e.target.value)}
                disabled={sectionMode !== null}
              >
                <option value="">{t('dialogs.task.noSection')}</option>
                {sectionsForList.map((section) => (
                  <option key={section.id} value={section.id}>
                    {section.name}
                  </option>
                ))}
              </select>
              {/* Manage buttons — local lists only. Hidden while the
                  inline name editor is open. */}
              {sectionsManageable && sectionMode === null && (
                <>
                  <button
                    type="button"
                    className="section-field__button"
                    onClick={() => {
                      setSectionDraft('');
                      setSectionMode('create');
                    }}
                  >
                    {t('dialogs.task.section.add')}
                  </button>
                  {form.sectionId && (
                    <>
                      <button
                        type="button"
                        className="section-field__button"
                        onClick={() => {
                          const cur = sectionsForList.find(
                            (s) => s.id === form.sectionId,
                          );
                          setSectionDraft(cur?.name ?? '');
                          setSectionMode('rename');
                        }}
                      >
                        {t('dialogs.task.section.rename')}
                      </button>
                      <button
                        type="button"
                        className="section-field__button section-field__button--danger"
                        onClick={() => void deleteCurrentSection()}
                        disabled={sectionBusy}
                      >
                        {t('dialogs.task.section.delete')}
                      </button>
                    </>
                  )}
                </>
              )}
            </div>
            {sectionMode !== null && (
              <div className="section-field__edit">
                <input
                  type="text"
                  value={sectionDraft}
                  onChange={(e) => setSectionDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      void submitSection();
                    } else if (e.key === 'Escape') {
                      e.preventDefault();
                      setSectionMode(null);
                      setSectionDraft('');
                    }
                  }}
                  placeholder={t('dialogs.task.section.namePlaceholder')}
                  aria-label={
                    sectionMode === 'rename'
                      ? t('dialogs.task.section.renameLabel')
                      : t('dialogs.task.section.newLabel')
                  }
                  autoFocus
                  disabled={sectionBusy}
                />
                <button
                  type="button"
                  className="section-field__button"
                  onClick={() => void submitSection()}
                  disabled={sectionBusy || !sectionDraft.trim()}
                >
                  {sectionMode === 'rename'
                    ? t('dialogs.task.section.save')
                    : t('dialogs.task.section.create')}
                </button>
                <button
                  type="button"
                  className="section-field__button"
                  onClick={() => {
                    setSectionMode(null);
                    setSectionDraft('');
                  }}
                >
                  {t('dialogs.task.section.cancel')}
                </button>
              </div>
            )}
          </div>
        )}

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

        {/* Two independent date+time blocks. Either or both can be
            unset (empty date input). The time input next to each
            date is disabled when the date is empty — a time without
            a day is meaningless and would fail the DB CHECK
            constraint anyway. Clearing the date doesn't clear the
            time field's local string; the save path discards it when
            the date is empty, and restoring the date brings the
            previously-typed time back. */}
        <fieldset className="form__fieldset">
          <legend className="form__label">
            {t('dialogs.task.fields.scheduled.legend')}
          </legend>
          <p className="form__hint">
            {t('dialogs.task.fields.scheduled.hint')}
          </p>
          <div className="form__row">
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.fields.scheduled.date')}
              </span>
              <input
                type="date"
                value={form.scheduledDate}
                onChange={(e) => update('scheduledDate', e.target.value)}
              />
            </label>
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.fields.scheduled.time')}
              </span>
              <input
                type="time"
                value={form.scheduledTime}
                onChange={(e) => update('scheduledTime', e.target.value)}
                disabled={!form.scheduledDate}
                aria-disabled={!form.scheduledDate || undefined}
              />
            </label>
          </div>
        </fieldset>

        <fieldset className="form__fieldset">
          <legend className="form__label">
            {t('dialogs.task.fields.deadline.legend')}
          </legend>
          <p className="form__hint">
            {t('dialogs.task.fields.deadline.hint')}
          </p>
          <div className="form__row">
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.fields.deadline.date')}
              </span>
              <input
                type="date"
                value={form.deadlineDate}
                onChange={(e) => update('deadlineDate', e.target.value)}
              />
            </label>
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.task.fields.deadline.time')}
              </span>
              <input
                type="time"
                value={form.deadlineTime}
                onChange={(e) => update('deadlineTime', e.target.value)}
                disabled={!form.deadlineDate}
                aria-disabled={!form.deadlineDate || undefined}
              />
            </label>
          </div>
        </fieldset>

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
        <DescriptionLinks text={form.description} />

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

        {isEdit && task && (supportsSubtasks || subtasks.length > 0) && (
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
            {supportsSubtasks && (
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
            )}
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
    // Direct field-by-field mapping. Each slot is independently
    // optional — a task may have schedule only, deadline only, both
    // (Plan + Soft-Deadline), or neither (backlog).
    return {
      title: task.title,
      listId: task.list_id,
      sectionId: task.section_id ?? '',
      status: task.status,
      priority: task.priority,
      scheduledDate: task.scheduled_date ?? '',
      scheduledTime: task.scheduled_time?.slice(0, 5) ?? '',
      deadlineDate: task.deadline_date ?? '',
      deadlineTime: task.deadline_time?.slice(0, 5) ?? '',
      description: task.description ?? '',
      colorLabel: task.color_label ?? null,
      recurrence: recurrenceFromBackend(task.recurrence),
      reminders: task.reminders ?? [],
    };
  }
  // When the caller anchored us on a day, default to a scheduled
  // task on that day. When unset, the task starts dateless (backlog).
  const anchored = defaultDate ? defaultDate.slice(0, 10) : '';
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
    sectionId: '',
    status: 'open',
    priority: 'medium',
    scheduledDate: anchored,
    scheduledTime: '',
    deadlineDate: '',
    deadlineTime: '',
    description: '',
    colorLabel: null,
    recurrence: { ...TASK_RECURRENCE_DEFAULT },
    reminders: [],
  };
}

interface DeadlineFields {
  scheduled_date: string | null;
  scheduled_time: string | null;
  deadline_date: string | null;
  deadline_time: string | null;
}

/**
 * Walk the whole subtree rooted at `parentId` and return every
 * descendant — direct children plus grand-children, recursively.
 * Used by the list-change cascade so a parent moving to a new
 * task list takes its entire family along.
 *
 * Iterative to avoid stack issues on adversarial deep nesting,
 * even though the UI doesn't currently encourage trees beyond two
 * levels.
 */
/**
 * Apply a list of ancestor recompute writes — flips the `status` and
 * `completed_at` of each affected task. The cascade planner produces
 * the writes; this helper just executes them. Iterates the snapshot
 * so each Task object stays canonical (we only change two fields).
 */
async function applyAncestorWrites(
  writes: StatusWrite[],
  snapshot: Task[],
): Promise<void> {
  if (writes.length === 0) return;
  const byId = new Map(snapshot.map((row) => [row.id, row]));
  for (const w of writes) {
    const target = byId.get(w.taskId);
    if (!target) continue;
    await invoke<Task>('update_task', {
      task: {
        ...target,
        status: w.status,
        completed_at:
          w.status === 'completed' ? new Date().toISOString() : null,
        // Honour the planner's auto-date companion write — see the
        // matching block in useTaskStatusToggle.applyCascade.
        scheduled_date:
          w.scheduledDate !== undefined
            ? w.scheduledDate
            : target.scheduled_date,
      },
    });
  }
}

function collectDescendants(parentId: string, all: Task[]): Task[] {
  const out: Task[] = [];
  const stack: string[] = [parentId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    for (const t of all) {
      if (t.parent_id === id) {
        out.push(t);
        stack.push(t.id);
      }
    }
  }
  return out;
}

/**
 * Project the form's two date+time pairs onto the wire shape. Both
 * slots are independent: each is "set" only if the date is non-empty.
 * Time without a date silently drops — the UI prevents this, but the
 * helper stays defensive.
 *
 * Time values come in as `HH:MM`; the wire uses `HH:MM:SS`.
 */
function splitDeadline(form: FormState): DeadlineFields {
  const scheduledDate = form.scheduledDate || null;
  const scheduledTime =
    scheduledDate && form.scheduledTime
      ? `${form.scheduledTime}:00`
      : null;
  const deadlineDate = form.deadlineDate || null;
  const deadlineTime =
    deadlineDate && form.deadlineTime
      ? `${form.deadlineTime}:00`
      : null;
  return {
    scheduled_date: scheduledDate,
    scheduled_time: scheduledTime,
    deadline_date: deadlineDate,
    deadline_time: deadlineTime,
  };
}

