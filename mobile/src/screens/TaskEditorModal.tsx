import { DateTimePicker } from '@expo/ui/community/datetime-picker';
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type {
  Reminder,
  Task,
  TaskEffort,
  TaskPriority,
  TaskRecurrenceValue,
  TaskStatus,
  TaskUser,
} from '@aperio/shared';
import {
  TASK_RECURRENCE_DEFAULT,
  fromBackend,
  selfAssignOnStatusChange,
  toBackend,
} from '@aperio/shared';

import {
  createTask,
  getTaskById,
  getTasks,
  taskCurrentUser,
  taskListMembers,
  updateTask,
} from '../api/client';
import { AssigneePicker } from '../components/AssigneePicker';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { DescriptionLinks } from '../components/DescriptionLinks';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { RemindersEditor } from '../components/RemindersEditor';
import { SegmentedSelect } from '../components/SegmentedSelect';
import { SoundSelect } from '../components/SoundSelect';
import { SubtaskSection } from '../components/SubtaskSection';
import { TaskRecurrenceSelector } from '../components/TaskRecurrenceSelector';
import { useCancelHeader } from '../components/useCancelHeader';
import {
  formatLocalDate,
  formatLocalTime,
  parseLocalDate,
  parseLocalTime,
} from '../intl/dateTimeField';
import { currentUserForList } from '../state/currentUser';
import { writeLastUsedTaskList } from '../state/lastUsedTaskList';
import { readTaskBehaviour } from '../state/taskBehaviour';
import { useSoundPref } from '../state/useSoundPref';
import { useTaskStore } from '../state/taskStoreContext';
import {
  cascadeEditorStatus,
  collectDescendants,
  recomputeAncestors,
} from '../state/taskToggle';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// The rich task editor — a faithful RN port of the desktop TaskDialog, sub-4
// CORE: title, list, section, status, priority, scheduled + deadline date/time,
// description (+ a read-only "completed on" line). Recurrence and reminders are
// sub-4b; assignees / per-task sound / colour label stay desktop-only or are
// preserved-as-read on edit. Pure JS — no native bridge change: it assembles a
// full CreateTaskRequest / Task that round-trips through the existing JSON
// bridge. Every picker is an accessible RadioGroup (RN has no native <select>).

interface FormState {
  title: string;
  listId: string;
  sectionId: string; // '' = ungrouped
  status: TaskStatus;
  priority: TaskPriority;
  effort: TaskEffort;
  scheduledDate: string;
  scheduledTime: string;
  deadlineDate: string;
  deadlineTime: string;
  /** Per-task override for how many days before the deadline the early
   *  reminder fires. `null` ⇒ use the global `tasks.deadlineCountdownDays`.
   *  Only meaningful while a deadline date is set. */
  deadlineReminderDays: number | null;
  description: string;
  recurrence: TaskRecurrenceValue;
  reminders: Reminder[];
  colorLabel: string; // '' = no colour
  assignees: TaskUser[]; // empty on local lists / providers without sharing
}

function buildInitialState(
  loaded: Task | null,
  listId: string,
  seed?: { title?: string; scheduledDate?: string },
): FormState {
  if (!loaded) {
    return {
      title: seed?.title ?? '',
      listId,
      sectionId: '',
      status: 'open',
      priority: 'medium',
      effort: 'medium',
      scheduledDate: seed?.scheduledDate ?? '',
      scheduledTime: '',
      deadlineDate: '',
      deadlineTime: '',
      deadlineReminderDays: null,
      description: '',
      recurrence: { ...TASK_RECURRENCE_DEFAULT },
      reminders: [],
      colorLabel: '',
      assignees: [],
    };
  }
  return {
    title: loaded.title,
    listId: loaded.list_id,
    sectionId: loaded.section_id ?? '',
    status: loaded.status,
    priority: loaded.priority,
    effort: loaded.effort,
    scheduledDate: loaded.scheduled_date ?? '',
    scheduledTime: loaded.scheduled_time ? loaded.scheduled_time.slice(0, 5) : '',
    deadlineDate: loaded.deadline_date ?? '',
    deadlineTime: loaded.deadline_time ? loaded.deadline_time.slice(0, 5) : '',
    deadlineReminderDays: loaded.deadline_reminder_days,
    description: loaded.description ?? '',
    recurrence: fromBackend(loaded.recurrence),
    reminders: loaded.reminders ?? [],
    colorLabel: loaded.color_label ?? '',
    assignees: loaded.assignees ?? [],
  };
}

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
const TIME_RE = /^\d{2}:\d{2}$/;

/** Day presets offered for the per-task "remind me N days before the deadline"
 *  override. `null` (no override) is the implicit default and uses the global
 *  `tasks.deadlineCountdownDays`. */
const DEADLINE_REMINDER_PRESETS = [1, 2, 3, 5, 7, 14] as const;
/** Sentinel value for the "Default (global)" radio option; maps to/from `null`.
 *  Zero is safe — "0 days before" is never a real preset. */
const DEADLINE_REMINDER_DEFAULT = 0;

/** A day/time pair → stored shape: no date ⇒ both null (the DB CHECK forbids a
 *  time without a date); a `HH:MM` time is stored as `HH:MM:00`. */
function toStored(date: string, time: string): {
  date: string | null;
  time: string | null;
} {
  const d = date.trim();
  if (!d) return { date: null, time: null };
  const t = time.trim();
  return { date: d, time: t ? `${t}:00` : null };
}

/** True when a non-empty date is malformed in shape OR calendar value (month
 *  13, day 32, Feb 30). Empty is valid (the slot is simply unset). Shared by
 *  the scheduled/deadline slots and the recurrence UNTIL date. */
function dateInvalid(date: string): boolean {
  const d = date.trim();
  if (!d) return false;
  if (!DATE_RE.test(d)) return true;
  const [y, m, day] = d.split('-').map(Number);
  const probe = new Date(y, m - 1, day);
  return (
    probe.getFullYear() !== y ||
    probe.getMonth() !== m - 1 ||
    probe.getDate() !== day
  );
}

/** True when a non-empty date/time slot is malformed — guards the bridge from a
 *  value the Rust serde / DB CHECK would reject, so the user gets the localized
 *  message instead of a raw bridge error. Like `toStored`, only judges the time
 *  when a date is present (an orphan time is dropped, not an error). */
function slotInvalid(date: string, time: string): boolean {
  const d = date.trim();
  const t = time.trim();
  if (d) {
    if (dateInvalid(d)) return true;
    if (t) {
      if (!TIME_RE.test(t)) return true;
      const [hh, mm] = t.split(':').map(Number);
      if (hh > 23 || mm > 59) return true;
    }
  }
  return false;
}

export default function TaskEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'TaskEditor'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { taskId, listId, parentId, initialTitle, initialScheduledDate } = route.params;
  const { taskLists, sectionsByList, loadSections, colorLabels, invalidateData } =
    useTaskStore();

  const [form, setForm] = useState<FormState>(() =>
    buildInitialState(null, listId, {
      title: initialTitle,
      scheduledDate: initialScheduledDate,
    }),
  );
  const [loaded, setLoaded] = useState<Task | null>(null);
  const [loading, setLoading] = useState(taskId != null);
  const [error, setError] = useState<string | null>(null);
  // The selected list's assignable member pool + the connected account's own id
  // ("me"), for the assignee picker. Both empty/null for local lists and
  // providers without sharing — the picker is then hidden (§9.7).
  const [members, setMembers] = useState<TaskUser[]>([]);
  const [currentUserId, setCurrentUserId] = useState<string | null>(null);
  // Create mode only: subtask titles staged on the form, written right after the
  // parent on Save (the parent has no id to reference yet). Edit mode manages
  // real subtasks live via SubtaskSection instead.
  const [draftSubtasks, setDraftSubtasks] = useState<string[]>([]);
  const [newSubtaskTitle, setNewSubtaskTitle] = useState('');
  // Per-task sound OVERRIDE (§14.4 item level) — a host-local `sound.item.{id}`
  // pref, NOT the inline Task.sound (which the reminder resolver ignores).
  // Edit-only: a new task has no id to key the pref on yet, so it inherits the
  // list/global default until re-edited (matches the desktop).
  const itemSound = useSoundPref(loaded ? `sound.item.${loaded.id}` : null);

  const titleRef = useRef<TextInput | null>(null);
  const scheduledDateRef = useRef<View | null>(null);
  const deadlineDateRef = useRef<View | null>(null);

  // Header title (announced on present) reflects the mode.
  useEffect(() => {
    navigation.setOptions({
      title:
        taskId == null ? t('mobile.newTaskLabel') : t('mobile.editTaskLabel'),
    });
  }, [navigation, t, taskId]);
  // A Cancel button as the first header element so the user can back out
  // without swiping to the bottom of the form.
  useCancelHeader(navigation);

  // Every dismissal (Save, Cancel, header back, swipe) bumps the data version so
  // the list refetches — the desktop DialogState.close() behaviour.
  useEffect(() => {
    return () => invalidateData();
  }, [invalidateData]);

  // Edit mode: load the task and prefill. Create mode starts blank.
  useEffect(() => {
    if (taskId == null) return;
    let cancelled = false;
    void (async () => {
      try {
        const task = await getTaskById(taskId);
        if (cancelled) return;
        if (task == null) {
          setError(t('mobile.taskMissing'));
          AccessibilityInfo.announceForAccessibility(t('mobile.taskMissing'));
          return;
        }
        setLoaded(task);
        setForm(buildInitialState(task, listId));
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [taskId, listId, t]);

  // Move SR focus into the title field once content is ready (a modal must
  // drive focus explicitly or TalkBack lingers on the trigger row).
  useEffect(() => {
    if (loading) return;
    const tag = findNodeHandle(titleRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    // New task: also open the keyboard on the title so the user can start
    // typing immediately (the desktop autofocuses the title).
    if (taskId == null) titleRef.current?.focus();
  }, [loading, taskId]);

  // Make sure the selected list's sections are loaded so the section picker can
  // offer them (lazy + sticky in the store).
  useEffect(() => {
    if (form.listId && !(form.listId in sectionsByList)) {
      void loadSections(form.listId);
    }
  }, [form.listId, sectionsByList, loadSections]);

  // Load the selected list's assignable member pool + "me" for the assignee
  // picker. Both come back empty/null for local lists + providers without
  // sharing (the picker is then hidden), so this runs for any list; a stale
  // in-flight result from a previous list is ignored.
  useEffect(() => {
    const list = form.listId;
    if (!list) {
      setMembers([]);
      setCurrentUserId(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const [pool, me] = await Promise.all([
          taskListMembers(list),
          taskCurrentUser(list),
        ]);
        if (cancelled) return;
        setMembers(pool);
        setCurrentUserId(me?.id ?? null);
      } catch {
        if (!cancelled) {
          setMembers([]);
          setCurrentUserId(null);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [form.listId]);

  const update = useCallback(
    <K extends keyof FormState>(key: K, value: FormState[K]) =>
      setForm((f) => ({ ...f, [key]: value })),
    [],
  );

  // Whether the user has manually picked a status this session. When they
  // haven't, the form's status FOLLOWS a subtask cascade (see syncParent) so
  // ticking a child to completion and then saving the parent doesn't revert the
  // derived status; once they pick a status explicitly, their choice is honoured.
  const statusTouched = useRef(false);
  const changeStatus = useCallback(
    (value: TaskStatus) => {
      statusTouched.current = true;
      setForm((f) => ({ ...f, status: value }));
    },
    [],
  );

  // Adopt a subtask-cascade-updated parent: `loaded` (the persisted truth) takes
  // the new status + completion stamp so Save's `...loaded` spread round-trips
  // them; the form's status follows only when the user hasn't diverged. Stable
  // (functional updates + a ref) so SubtaskSection's reload can't loop on it.
  const syncParent = useCallback((parent: Task) => {
    setLoaded((prev) =>
      prev == null ||
      (prev.status === parent.status && prev.completed_at === parent.completed_at)
        ? prev
        : { ...prev, status: parent.status, completed_at: parent.completed_at },
    );
    if (!statusTouched.current) {
      setForm((f) => (f.status === parent.status ? f : { ...f, status: parent.status }));
    }
  }, []);

  const changeList = useCallback(
    (nextListId: string) => {
      setForm((f) => {
        const secs = sectionsByList[nextListId] ?? [];
        const keep = secs.some((s) => s.id === f.sectionId);
        // A different list means a different member pool, so drop the assignees
        // (the new list's picker re-offers from its own pool).
        return {
          ...f,
          listId: nextListId,
          sectionId: keep ? f.sectionId : '',
          assignees: [],
        };
      });
      if (!(nextListId in sectionsByList)) void loadSections(nextListId);
    },
    [sectionsByList, loadSections],
  );

  const clearSlot = useCallback(
    (which: 'scheduled' | 'deadline') => {
      setForm((f) =>
        which === 'scheduled'
          ? { ...f, scheduledDate: '', scheduledTime: '' }
          : // Drop the per-task reminder override alongside the deadline — it
            // is meaningless without a deadline (mirrors deadlineTime).
            { ...f, deadlineDate: '', deadlineTime: '', deadlineReminderDays: null },
      );
      AccessibilityInfo.announceForAccessibility(
        t(`dialogs.task.fields.${which}.cleared`),
      );
      const ref = which === 'scheduled' ? scheduledDateRef : deadlineDateRef;
      const tag = findNodeHandle(ref.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    },
    [t],
  );

  const listOptions = useMemo(
    () => taskLists.map((l) => ({ value: l.id, label: l.name })),
    [taskLists],
  );
  const sectionOptions = useMemo(
    () => [
      { value: '', label: t('dialogs.task.noSection') },
      ...(sectionsByList[form.listId] ?? []).map((s) => ({
        value: s.id,
        label: s.name,
      })),
    ],
    [sectionsByList, form.listId, t],
  );
  // Gate affordances on the selected list's adapter capabilities so we never
  // offer a control whose value the backend would silently drop on save (e.g.
  // recurrence on a backend without task recurrence, sections on a flat one).
  // Absent caps default permissive (cal-core-native) — matches the prior
  // always-show behaviour; the Host now stamps real caps on every list.
  const caps = useMemo(
    () => taskLists.find((l) => l.id === form.listId)?.task_capabilities,
    [taskLists, form.listId],
  );
  const canRecur = caps?.task_recurrence ?? true;
  const recurrenceCaps = useMemo(
    () => taskLists.find((l) => l.id === form.listId)?.recurrence_capabilities,
    [taskLists, form.listId],
  );
  const canSection = caps?.sections ?? true;
  // Colour binds to a LOCAL task on its own row; on an external task it would be
  // a host-local override (a later increment), so only offer it for local lists.
  const isLocalList = useMemo(
    () => taskLists.find((l) => l.id === form.listId)?.account_id === 'local',
    [taskLists, form.listId],
  );
  // The loaded task's OWN list (its children live there) — for the live subtask
  // editor's `supports_in_progress` + per-list cascade resolution.
  const loadedList = useMemo(
    () => (loaded ? taskLists.find((l) => l.id === loaded.list_id) : undefined),
    [loaded, taskLists],
  );
  // Subtasks ride parent_id on the LOCAL store; mirror TasksScreen's gate
  // (account_id === 'local'). Edit mode keys off the LOADED task's list (where
  // its children actually live) so an unsaved list-picker change doesn't hide
  // the live manager; create mode stages drafts against the form's chosen list
  // (but not when creating a subtask — no nested-draft staging).
  const showSubtaskEditor =
    taskId != null && loaded != null && loadedList?.account_id === 'local';
  const showDraftSubtasks = taskId == null && isLocalList && parentId == null;

  const addDraftSubtask = useCallback(() => {
    const trimmed = newSubtaskTitle.trim();
    if (!trimmed) return;
    setDraftSubtasks((d) => [...d, trimmed]);
    setNewSubtaskTitle('');
    AccessibilityInfo.announceForAccessibility(
      t('dialogs.task.subtasks.added', { title: trimmed }),
    );
  }, [newSubtaskTitle, t]);

  const removeDraftSubtask = useCallback(
    (index: number) => {
      const removed = draftSubtasks[index];
      setDraftSubtasks((d) => d.filter((_, j) => j !== index));
      if (removed) {
        AccessibilityInfo.announceForAccessibility(
          t('dialogs.task.deleted', { title: removed }),
        );
      }
    },
    [draftSubtasks, t],
  );
  const statusOptions = useMemo(
    () => [
      { value: 'open' as const, label: t('dialogs.task.status.open') },
      { value: 'in_progress' as const, label: t('dialogs.task.status.inProgress') },
      { value: 'completed' as const, label: t('dialogs.task.status.completed') },
      { value: 'cancelled' as const, label: t('dialogs.task.status.cancelled') },
    ],
    [t],
  );
  const priorityOptions = useMemo(
    () => [
      { value: 'low' as const, label: t('dialogs.task.priority.low') },
      { value: 'medium' as const, label: t('dialogs.task.priority.medium') },
      { value: 'high' as const, label: t('dialogs.task.priority.high') },
    ],
    [t],
  );
  const effortOptions = useMemo(
    () => [
      { value: 'small' as const, label: t('dialogs.task.effort.small') },
      { value: 'medium' as const, label: t('dialogs.task.effort.medium') },
      { value: 'large' as const, label: t('dialogs.task.effort.large') },
    ],
    [t],
  );
  // "Default (global)" plus the day presets. Modelled as a RadioGroup (not a
  // segmented control) — seven options is too many for a segment row, and each
  // radio is its own focus stop for a screen-reader user.
  const deadlineReminderOptions = useMemo(() => {
    // The day presets, plus the current value spliced in (sorted) when it's a
    // non-null override that isn't a preset — e.g. a `10` synced from another
    // device. Without this the RadioGroup announces NOTHING selected for such a
    // value, even though it saves back intact.
    const set = new Set<number>(DEADLINE_REMINDER_PRESETS);
    if (form.deadlineReminderDays != null) {
      set.add(form.deadlineReminderDays);
    }
    const days = [...set].sort((a, b) => a - b);
    return [
      {
        value: DEADLINE_REMINDER_DEFAULT,
        label: t('dialogs.task.fields.deadlineReminder.default'),
      },
      ...days.map((d) => ({
        value: d,
        label: t('dialogs.task.fields.deadlineReminder.option', {
          count: d,
        }),
      })),
    ];
  }, [t, form.deadlineReminderDays]);

  // A subtask must stay in its parent's list (the bridge has no list-move hint)
  // — both when editing an existing subtask and when creating a new one.
  const listLocked = loaded?.parent_id != null || parentId != null;
  const blocked = taskId != null && loaded == null;

  const completedLine = useMemo(() => {
    if (form.status !== 'completed' || !loaded?.completed_at) return null;
    const fmt = new Intl.DateTimeFormat(i18n.language, {
      dateStyle: 'long',
      timeStyle: 'short',
    });
    return t('dialogs.task.fields.completedAt', {
      date: fmt.format(new Date(loaded.completed_at)),
    });
  }, [form.status, loaded, i18n.language, t]);

  const save = useCallback(async () => {
    const title = form.title.trim();
    if (!title) {
      setError(t('dialogs.task.titleRequired'));
      AccessibilityInfo.announceForAccessibility(t('dialogs.task.titleRequired'));
      return;
    }
    if (!form.listId) {
      setError(t('dialogs.task.listRequired'));
      AccessibilityInfo.announceForAccessibility(t('dialogs.task.listRequired'));
      return;
    }
    if (
      slotInvalid(form.scheduledDate, form.scheduledTime) ||
      slotInvalid(form.deadlineDate, form.deadlineTime) ||
      // The recurrence UNTIL date is free-text too; a malformed one would fail
      // serde at the bridge (RecurrenceEnd::OnDate is a NaiveDate). Skipped when
      // the list can't store recurrence (the rule is dropped on save anyway).
      (canRecur &&
        form.recurrence.freq !== 'NONE' &&
        form.recurrence.endMode === 'UNTIL' &&
        dateInvalid(form.recurrence.until))
    ) {
      setError(t('mobile.invalidDateTime'));
      AccessibilityInfo.announceForAccessibility(t('mobile.invalidDateTime'));
      return;
    }
    const sched = toStored(form.scheduledDate, form.scheduledTime);
    const dead = toStored(form.deadlineDate, form.deadlineTime);
    const description = form.description.trim() || null;
    setError(null);
    try {
      if (taskId == null) {
        const created = await createTask({
          list_id: form.listId,
          title,
          description,
          status: form.status,
          priority: form.priority,
          effort: form.effort,
          scheduled_date: sched.date,
          scheduled_time: sched.time,
          deadline_date: dead.date,
          deadline_time: dead.time,
          // Per-task reminder override only rides along with a real deadline;
          // a deadline-less task always falls back to the global countdown.
          deadline_reminder_days: dead.date ? form.deadlineReminderDays : null,
          recurrence: canRecur ? toBackend(form.recurrence) : null,
          parent_id: parentId ?? null,
          section_id: canSection ? form.sectionId || null : null,
          color_label: isLocalList ? form.colorLabel || null : null,
          reminders: form.reminders,
          // Rides through to a sharing-capable external adapter; the local store
          // ignores it (the picker is hidden for local lists, so it's empty there).
          assignees: form.assignees,
          sound: null,
        });
        // Write any staged draft subtasks under the freshly-created parent (they
        // default to open/medium and inherit the parent's section, matching the
        // edit-mode add), then recompute the new parent's derived status.
        if (draftSubtasks.length > 0) {
          for (const subTitle of draftSubtasks) {
            await createTask({
              list_id: created.list_id,
              title: subTitle,
              description: null,
              status: 'open',
              priority: 'medium',
              effort: 'medium',
              scheduled_date: null,
              scheduled_time: null,
              deadline_date: null,
              deadline_time: null,
              deadline_reminder_days: null,
              recurrence: null,
              parent_id: created.id,
              section_id: created.section_id,
              color_label: null,
              reminders: [],
              assignees: [],
              sound: null,
            });
          }
          await recomputeAncestors(created.id, await getTasks(created.list_id));
        }
        // Adding a subtask can change the parent's derived status (e.g. an open
        // child re-opens a completed parent). Recompute ancestors against the
        // owning list's post-create snapshot, honouring the coupling knob.
        if (parentId != null) {
          await recomputeAncestors(parentId, await getTasks(created.list_id));
        }
        // Remember the chosen list so the next quick-add / new-task defaults to
        // it (a top-level create; a subtask's list is locked to its parent's).
        if (parentId == null) await writeLastUsedTaskList(form.listId);
        AccessibilityInfo.announceForAccessibility(
          t('mobile.added', { title }),
        );
      } else if (loaded != null) {
        // A status change, or a (non-subtask) list change, must cascade to the
        // family — capture the list's pre-edit task set before writing the root.
        const listChanged =
          loaded.parent_id == null &&
          parentId == null &&
          form.listId !== loaded.list_id;
        const statusChanged = form.status !== loaded.status;
        const familyTasks =
          listChanged || statusChanged ? await getTasks(loaded.list_id) : [];
        // Editor self-assign (shared lists): a status change here applies the
        // check-off rule to the ROOT too — assign me on →in_progress/→completed
        // of an unassigned task, drop only me on →open. Gated on a real status
        // change; the family (children) self-assign via cascadeEditorStatus below.
        let rootAssignees = form.assignees;
        if (statusChanged) {
          const behaviour = await readTaskBehaviour();
          const me = behaviour.autoSelfAssign
            ? await currentUserForList(form.listId)
            : null;
          rootAssignees =
            selfAssignOnStatusChange(
              form.status,
              form.assignees,
              me,
              behaviour.autoSelfAssign,
            ) ?? form.assignees;
        }
        // Spread ...loaded so store-managed fields (series_id, resurface_date,
        // etag, created_at) AND the not-yet-editable ones (per-task sound)
        // round-trip untouched; the edited fields below (incl. assignees) win.
        // Pass loaded.list_id as the previous list so a list-picker change is
        // detected as a cross-list move (create-on-target + delete-from-source)
        // rather than an in-place PATCH at the wrong resource (412/404 external).
        await updateTask(
          {
            ...loaded,
            title,
            list_id: form.listId,
            // A cross-list move drops the section (it belonged to the old list);
            // a list with no sections drops it too.
            section_id:
              !canSection || form.listId !== loaded.list_id
                ? null
                : form.sectionId || null,
            status: form.status,
            priority: form.priority,
            effort: form.effort,
            scheduled_date: sched.date,
            scheduled_time: sched.time,
            deadline_date: dead.date,
            deadline_time: dead.time,
            // Per-task reminder override only rides along with a real deadline;
            // a deadline-less task always falls back to the global countdown.
            deadline_reminder_days: dead.date ? form.deadlineReminderDays : null,
            recurrence: canRecur ? toBackend(form.recurrence) : null,
            reminders: form.reminders,
            assignees: rootAssignees,
            description,
            // Local task: the picker drives the colour; external: leave the
            // (override-stamped) value untouched (external colour = later).
            color_label: isLocalList
              ? form.colorLabel || null
              : loaded.color_label,
            completed_at:
              form.status === 'completed'
                ? (loaded.completed_at ?? new Date().toISOString())
                : null,
          },
          loaded.list_id,
        );
        // Mirror the desktop TaskDialog: a parent's list change drags its whole
        // family along, and a status change made via the editor cascades through
        // the family (the root was just written above with its full field set).
        if (listChanged) {
          for (const child of collectDescendants(loaded.id, familyTasks)) {
            await updateTask({ ...child, list_id: form.listId }, child.list_id);
          }
        }
        if (statusChanged) {
          const moved = new Set(
            listChanged
              ? collectDescendants(loaded.id, familyTasks).map((c) => c.id)
              : [],
          );
          // The cascade snapshot reflects the post-edit family (root's new
          // status + new list, moved descendants' new list) so its writes never
          // revert the list move.
          const cascadeSnapshot = familyTasks.map((row) =>
            row.id === loaded.id
              ? { ...row, status: form.status, list_id: form.listId }
              : moved.has(row.id)
                ? { ...row, list_id: form.listId }
                : row,
          );
          await cascadeEditorStatus(
            loaded.id,
            form.status,
            form.listId,
            taskLists.find((l) => l.id === form.listId),
            cascadeSnapshot,
          );
        }
        AccessibilityInfo.announceForAccessibility(
          t('mobile.saved', { title }),
        );
      } else {
        return;
      }
      navigation.goBack();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      AccessibilityInfo.announceForAccessibility(t('mobile.error', { message }));
    }
  }, [
    canRecur,
    canSection,
    isLocalList,
    form,
    loaded,
    navigation,
    parentId,
    t,
    taskId,
    taskLists,
    draftSubtasks,
  ]);

  return (
    <FormScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      accessibilityViewIsModal
    >
      {error != null && (
        <Text
          style={styles.error}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {error}
        </Text>
      )}

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.task.fields.title')}</Text>
        <TextInput
          ref={titleRef}
          style={styles.input}
          value={form.title}
          onChangeText={(v) => update('title', v)}
          placeholder={t('mobile.newTaskPlaceholder')}
          accessibilityLabel={t('dialogs.task.fields.title')}
          editable={!loading}
          returnKeyType="next"
        />
      </View>

      <RadioGroup<string>
        label={t('dialogs.task.fields.list')}
        value={form.listId}
        options={listOptions}
        onChange={changeList}
        disabled={listLocked}
      />
      {listLocked && (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.task.subtaskListLocked')}
        </Text>
      )}

      {canSection && (
        <RadioGroup<string>
          label={t('dialogs.task.fields.section')}
          value={form.sectionId}
          options={sectionOptions}
          onChange={(v) => update('sectionId', v)}
        />
      )}

      {isLocalList && (
        <ColorLabelSelect
          value={form.colorLabel}
          labels={colorLabels}
          onChange={(v) => update('colorLabel', v)}
        />
      )}

      <SegmentedSelect<TaskStatus>
        label={t('dialogs.task.fields.status')}
        value={form.status}
        options={statusOptions}
        onChange={changeStatus}
      />
      {completedLine != null && (
        <Text style={styles.hint} accessibilityRole="text">
          {completedLine}
        </Text>
      )}

      <SegmentedSelect<TaskPriority>
        label={t('dialogs.task.fields.priority')}
        value={form.priority}
        options={priorityOptions}
        onChange={(v) => update('priority', v)}
      />

      <SegmentedSelect<TaskEffort>
        label={t('dialogs.task.fields.effort')}
        value={form.effort}
        options={effortOptions}
        onChange={(v) => update('effort', v)}
      />

      <DateTimeField
        legend={t('dialogs.task.fields.scheduled.legend')}
        hint={t('dialogs.task.fields.scheduled.hint')}
        dateLabel={t('dialogs.task.fields.scheduled.date')}
        timeLabel={t('dialogs.task.fields.scheduled.time')}
        addDateLabel={t('dialogs.task.fields.scheduled.addDate')}
        clearLabel={t('dialogs.task.fields.scheduled.clear')}
        addTimeLabel={t('dialogs.task.fields.scheduled.addTime')}
        clearTimeLabel={t('dialogs.task.fields.scheduled.clearTime')}
        dateValue={form.scheduledDate}
        timeValue={form.scheduledTime}
        onChangeDate={(v) => update('scheduledDate', v)}
        onChangeTime={(v) => update('scheduledTime', v)}
        onClear={() => clearSlot('scheduled')}
        fieldRef={scheduledDateRef}
        editable={!loading}
        locale={i18n.language}
      />

      <DateTimeField
        legend={t('dialogs.task.fields.deadline.legend')}
        hint={t('dialogs.task.fields.deadline.hint')}
        dateLabel={t('dialogs.task.fields.deadline.date')}
        timeLabel={t('dialogs.task.fields.deadline.time')}
        addDateLabel={t('dialogs.task.fields.deadline.addDate')}
        clearLabel={t('dialogs.task.fields.deadline.clear')}
        addTimeLabel={t('dialogs.task.fields.deadline.addTime')}
        clearTimeLabel={t('dialogs.task.fields.deadline.clearTime')}
        dateValue={form.deadlineDate}
        timeValue={form.deadlineTime}
        onChangeDate={(v) => update('deadlineDate', v)}
        onChangeTime={(v) => update('deadlineTime', v)}
        onClear={() => clearSlot('deadline')}
        fieldRef={deadlineDateRef}
        editable={!loading}
        locale={i18n.language}
      />

      {/* The per-task reminder override only matters when a deadline is set; it
          falls back to the global countdown otherwise, so it stays hidden. */}
      {form.deadlineDate !== '' && (
        <View style={styles.field}>
          <RadioGroup<number>
            label={t('dialogs.task.fields.deadlineReminder.label')}
            value={form.deadlineReminderDays ?? DEADLINE_REMINDER_DEFAULT}
            options={deadlineReminderOptions}
            onChange={(v) =>
              update(
                'deadlineReminderDays',
                v === DEADLINE_REMINDER_DEFAULT ? null : v,
              )
            }
            disabled={loading}
          />
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.task.fields.deadlineReminder.hint')}
          </Text>
        </View>
      )}

      <View style={styles.field}>
        <Text style={styles.legend}>{t('dialogs.task.fields.description')}</Text>
        <TextInput
          style={[styles.input, styles.multiline]}
          value={form.description}
          onChangeText={(v) => update('description', v)}
          accessibilityLabel={t('dialogs.task.fields.description')}
          editable={!loading}
          multiline
          numberOfLines={4}
          textAlignVertical="top"
        />
        <DescriptionLinks text={form.description} />
      </View>

      {canRecur && (
        <TaskRecurrenceSelector
          value={form.recurrence}
          onChange={(recurrence) => update('recurrence', recurrence)}
          capabilities={recurrenceCaps}
        />
      )}

      <RemindersEditor
        mode="task"
        value={form.reminders}
        onChange={(reminders) => update('reminders', reminders)}
      />

      {/* Per-task sound override (§14.4 item level) — edit-only (a new task has
          no id to key the pref on yet; it inherits until re-edited). */}
      {taskId != null && loaded != null && !itemSound.loading && (
        <SoundSelect
          label={t('reminders.sound.label')}
          value={itemSound.value}
          allowInherit
          onChange={(next) => void itemSound.save(next)}
        />
      )}

      {/* Assignees — only when the selected list has an assignable member pool
          (external, sharing-capable providers). Hidden for local lists. */}
      {members.length > 0 && (
        <AssigneePicker
          members={members}
          value={form.assignees}
          currentUserId={currentUserId}
          onChange={(next) => update('assignees', next)}
        />
      )}

      {/* Subtasks — edit mode manages real children live (mutations persist
          immediately); create mode stages draft titles written on Save. */}
      {showSubtaskEditor && loaded != null && (
        <SubtaskSection
          parentTask={loaded}
          list={loadedList}
          onChanged={invalidateData}
          onParentSync={syncParent}
        />
      )}

      {showDraftSubtasks && (
        <View style={styles.field}>
          <Text style={styles.legend} accessibilityRole="header">
            {t('dialogs.task.subtasks.heading')}
          </Text>
          {draftSubtasks.length === 0 ? (
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.task.subtasks.empty')}
            </Text>
          ) : (
            <View
              accessibilityRole="list"
              accessibilityLabel={t('dialogs.task.subtasks.heading')}
              style={styles.draftList}
            >
              {draftSubtasks.map((title, i) => (
                // Index-keyed: a staged-title list with no reordering.
                <View key={`${i}-${title}`} style={styles.draftRow}>
                  <Text
                    style={styles.draftTitle}
                    accessibilityRole="text"
                    accessibilityLabel={title}
                  >
                    {title}
                  </Text>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('dialogs.task.subtasks.removeAria', { title })}
                    onPress={() => removeDraftSubtask(i)}
                    style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
                  >
                    <Text style={styles.ghostButtonText}>{t('mobile.delete')}</Text>
                  </Pressable>
                </View>
              ))}
            </View>
          )}
          <View style={styles.subtaskAddRow}>
            <TextInput
              style={[styles.input, styles.subtaskInput]}
              value={newSubtaskTitle}
              onChangeText={setNewSubtaskTitle}
              placeholder={t('dialogs.task.subtasks.placeholder')}
              accessibilityLabel={t('dialogs.task.subtasks.newAria')}
              returnKeyType="done"
              onSubmitEditing={addDraftSubtask}
            />
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={t('dialogs.task.subtasks.addButton')}
              accessibilityState={{ disabled: newSubtaskTitle.trim() === '' }}
              disabled={newSubtaskTitle.trim() === ''}
              onPress={addDraftSubtask}
              style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
            >
              <Text style={styles.ghostButtonText}>{t('dialogs.task.subtasks.addButton')}</Text>
            </Pressable>
          </View>
        </View>
      )}

      <View style={styles.buttons}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.save')}
          accessibilityState={{ disabled: blocked }}
          disabled={blocked}
          onPress={() => void save()}
          style={({ pressed }) => [
            styles.button,
            pressed && styles.buttonPressed,
            blocked && styles.buttonDisabled,
          ]}
        >
          <Text style={styles.buttonText}>{t('mobile.save')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.cancel')}
          onPress={() => navigation.goBack()}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.cancel')}</Text>
        </Pressable>
      </View>
    </FormScrollView>
  );
}

function DateTimeField({
  legend,
  hint,
  dateLabel,
  timeLabel,
  addDateLabel,
  clearLabel,
  addTimeLabel,
  clearTimeLabel,
  dateValue,
  timeValue,
  onChangeDate,
  onChangeTime,
  onClear,
  fieldRef,
  editable,
  locale,
}: {
  legend: string;
  hint: string;
  dateLabel: string;
  timeLabel: string;
  addDateLabel: string;
  clearLabel: string;
  addTimeLabel: string;
  clearTimeLabel: string;
  dateValue: string;
  timeValue: string;
  onChangeDate: (v: string) => void;
  onChangeTime: (v: string) => void;
  onClear: () => void;
  fieldRef: RefObject<View | null>;
  editable: boolean;
  locale: string;
}) {
  const styles = useThemedStyles(makeStyles);
  const hasDate = dateValue.trim() !== '';
  const hasTime = timeValue.trim() !== '';
  // Native date/time pickers (the same compact @expo/ui control the calendar
  // jump-to-date uses), mounted on demand: no day until the user adds one, no
  // time slot until they add that. The form still holds 'YYYY-MM-DD' / 'HH:MM'
  // strings, so save()/toStored() are unchanged — only the input UI differs.
  return (
    <View style={styles.field} ref={fieldRef}>
      <Text style={styles.legend}>{legend}</Text>
      {hint !== '' && (
        <Text style={styles.hint} accessibilityRole="text">
          {hint}
        </Text>
      )}
      {!hasDate ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${legend} – ${addDateLabel}`}
          accessibilityState={{ disabled: !editable }}
          disabled={!editable}
          onPress={() => onChangeDate(formatLocalDate(new Date()))}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
        >
          <Text style={styles.ghostButtonText}>{addDateLabel}</Text>
        </Pressable>
      ) : (
        <>
          <View style={styles.pickerRow}>
            <Text style={styles.pickerLabel}>{`${legend} – ${dateLabel}`}</Text>
            <DateTimePicker
              mode="date"
              display="compact"
              value={parseLocalDate(dateValue)}
              onValueChange={(_, d) => onChangeDate(formatLocalDate(d))}
              locale={locale}
            />
          </View>
          {!hasTime ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={`${legend} – ${addTimeLabel}`}
              accessibilityState={{ disabled: !editable }}
              disabled={!editable}
              onPress={() => onChangeTime(formatLocalTime(new Date()))}
              style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
            >
              <Text style={styles.ghostButtonText}>{addTimeLabel}</Text>
            </Pressable>
          ) : (
            <View style={styles.pickerRow}>
              <Text style={styles.pickerLabel}>{`${legend} – ${timeLabel}`}</Text>
              <DateTimePicker
                mode="time"
                display="compact"
                value={parseLocalTime(timeValue)}
                onValueChange={(_, d) => onChangeTime(formatLocalTime(d))}
                locale={locale}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`${legend} – ${clearTimeLabel}`}
                onPress={() => onChangeTime('')}
                style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
              >
                <Text style={styles.ghostButtonText}>{clearTimeLabel}</Text>
              </Pressable>
            </View>
          )}
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={clearLabel}
            onPress={onClear}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
          >
            <Text style={styles.ghostButtonText}>{clearLabel}</Text>
          </Pressable>
        </>
      )}
    </View>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 20, gap: 18 },
    field: { gap: 6 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    pickerRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 4,
      flexWrap: 'wrap',
    },
    pickerLabel: { fontSize: 16, color: c.textPrimary, flexShrink: 1 },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    multiline: { minHeight: 96 },
    draftList: { gap: 8 },
    draftRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    draftTitle: { flex: 1, fontSize: 16, color: c.textPrimary },
    subtaskAddRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
    subtaskInput: { flex: 1 },
    buttons: { flexDirection: 'row', gap: 10, marginTop: 8 },
    button: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonDisabled: { opacity: 0.5 },
    buttonText: { fontSize: 17, fontWeight: '700', color: c.textOnAccent },
    ghostButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostPressed: { backgroundColor: c.surfacePressed },
    ghostButtonText: { fontSize: 17, fontWeight: '600', color: c.link },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
