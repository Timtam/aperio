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
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type {
  Reminder,
  Task,
  TaskPriority,
  TaskRecurrenceValue,
  TaskStatus,
} from '@aperio/shared';
import { TASK_RECURRENCE_DEFAULT, fromBackend, toBackend } from '@aperio/shared';

import { createTask, getTaskById, updateTask } from '../api/client';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { RadioGroup } from '../components/RadioGroup';
import { RemindersEditor } from '../components/RemindersEditor';
import { TaskRecurrenceSelector } from '../components/TaskRecurrenceSelector';
import { useTaskStore } from '../state/taskStoreContext';
import type { RootStackScreenProps } from '../navigation/types';

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
  scheduledDate: string;
  scheduledTime: string;
  deadlineDate: string;
  deadlineTime: string;
  description: string;
  recurrence: TaskRecurrenceValue;
  reminders: Reminder[];
  colorLabel: string; // '' = no colour
}

function buildInitialState(loaded: Task | null, listId: string): FormState {
  if (!loaded) {
    return {
      title: '',
      listId,
      sectionId: '',
      status: 'open',
      priority: 'medium',
      scheduledDate: '',
      scheduledTime: '',
      deadlineDate: '',
      deadlineTime: '',
      description: '',
      recurrence: { ...TASK_RECURRENCE_DEFAULT },
      reminders: [],
      colorLabel: '',
    };
  }
  return {
    title: loaded.title,
    listId: loaded.list_id,
    sectionId: loaded.section_id ?? '',
    status: loaded.status,
    priority: loaded.priority,
    scheduledDate: loaded.scheduled_date ?? '',
    scheduledTime: loaded.scheduled_time ? loaded.scheduled_time.slice(0, 5) : '',
    deadlineDate: loaded.deadline_date ?? '',
    deadlineTime: loaded.deadline_time ? loaded.deadline_time.slice(0, 5) : '',
    description: loaded.description ?? '',
    recurrence: fromBackend(loaded.recurrence),
    reminders: loaded.reminders ?? [],
    colorLabel: loaded.color_label ?? '',
  };
}

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
const TIME_RE = /^\d{2}:\d{2}$/;

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
  const { taskId, listId } = route.params;
  const { taskLists, sectionsByList, loadSections, colorLabels, invalidateData } =
    useTaskStore();

  const [form, setForm] = useState<FormState>(() =>
    buildInitialState(null, listId),
  );
  const [loaded, setLoaded] = useState<Task | null>(null);
  const [loading, setLoading] = useState(taskId != null);
  const [error, setError] = useState<string | null>(null);

  const titleRef = useRef<TextInput | null>(null);
  const scheduledDateRef = useRef<TextInput | null>(null);
  const deadlineDateRef = useRef<TextInput | null>(null);

  // Header title (announced on present) reflects the mode.
  useEffect(() => {
    navigation.setOptions({
      title:
        taskId == null ? t('mobile.newTaskLabel') : t('mobile.editTaskLabel'),
    });
  }, [navigation, t, taskId]);

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
  }, [loading]);

  // Make sure the selected list's sections are loaded so the section picker can
  // offer them (lazy + sticky in the store).
  useEffect(() => {
    if (form.listId && !(form.listId in sectionsByList)) {
      void loadSections(form.listId);
    }
  }, [form.listId, sectionsByList, loadSections]);

  const update = useCallback(
    <K extends keyof FormState>(key: K, value: FormState[K]) =>
      setForm((f) => ({ ...f, [key]: value })),
    [],
  );

  const changeList = useCallback(
    (nextListId: string) => {
      setForm((f) => {
        const secs = sectionsByList[nextListId] ?? [];
        const keep = secs.some((s) => s.id === f.sectionId);
        return { ...f, listId: nextListId, sectionId: keep ? f.sectionId : '' };
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
          : { ...f, deadlineDate: '', deadlineTime: '' },
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
  const canSection = caps?.sections ?? true;
  // Colour binds to a LOCAL task on its own row; on an external task it would be
  // a host-local override (a later increment), so only offer it for local lists.
  const isLocalList = useMemo(
    () => taskLists.find((l) => l.id === form.listId)?.account_id === 'local',
    [taskLists, form.listId],
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

  // A subtask must stay in its parent's list (the bridge has no list-move hint).
  const listLocked = loaded?.parent_id != null;
  const blocked = taskId != null && loaded == null;

  const completedLine = useMemo(() => {
    if (form.status !== 'completed' || !loaded?.completed_at) return null;
    const fmt = new Intl.DateTimeFormat(i18n.language, {
      dateStyle: 'medium',
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
        await createTask({
          list_id: form.listId,
          title,
          description,
          status: form.status,
          priority: form.priority,
          scheduled_date: sched.date,
          scheduled_time: sched.time,
          deadline_date: dead.date,
          deadline_time: dead.time,
          recurrence: canRecur ? toBackend(form.recurrence) : null,
          parent_id: null,
          section_id: canSection ? form.sectionId || null : null,
          color_label: isLocalList ? form.colorLabel || null : null,
          reminders: form.reminders,
          assignees: [],
          sound: null,
        });
        AccessibilityInfo.announceForAccessibility(
          t('mobile.added', { title }),
        );
      } else if (loaded != null) {
        // Spread ...loaded so store-managed fields (series_id, resurface_date,
        // etag, created_at) AND the not-yet-editable ones (recurrence, reminders,
        // sound, colour, assignees — sub-4b/desktop) round-trip untouched.
        await updateTask({
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
          scheduled_date: sched.date,
          scheduled_time: sched.time,
          deadline_date: dead.date,
          deadline_time: dead.time,
          recurrence: canRecur ? toBackend(form.recurrence) : null,
          reminders: form.reminders,
          description,
          // Local task: the picker drives the colour; external: leave the
          // (override-stamped) value untouched (external colour = later).
          color_label: isLocalList ? form.colorLabel || null : loaded.color_label,
          completed_at:
            form.status === 'completed'
              ? (loaded.completed_at ?? new Date().toISOString())
              : null,
        });
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
  }, [canRecur, canSection, isLocalList, form, loaded, navigation, t, taskId]);

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
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

      <RadioGroup<TaskStatus>
        label={t('dialogs.task.fields.status')}
        value={form.status}
        options={statusOptions}
        onChange={(v) => update('status', v)}
      />
      {completedLine != null && (
        <Text style={styles.hint} accessibilityRole="text">
          {completedLine}
        </Text>
      )}

      <RadioGroup<TaskPriority>
        label={t('dialogs.task.fields.priority')}
        value={form.priority}
        options={priorityOptions}
        onChange={(v) => update('priority', v)}
      />

      <DateTimeField
        legend={t('dialogs.task.fields.scheduled.legend')}
        hint={t('dialogs.task.fields.scheduled.hint')}
        dateLabel={t('dialogs.task.fields.scheduled.date')}
        timeLabel={t('dialogs.task.fields.scheduled.time')}
        clearLabel={t('dialogs.task.fields.scheduled.clear')}
        dateValue={form.scheduledDate}
        timeValue={form.scheduledTime}
        onChangeDate={(v) => update('scheduledDate', v)}
        onChangeTime={(v) => update('scheduledTime', v)}
        onClear={() => clearSlot('scheduled')}
        dateRef={scheduledDateRef}
        editable={!loading}
      />

      <DateTimeField
        legend={t('dialogs.task.fields.deadline.legend')}
        hint={t('dialogs.task.fields.deadline.hint')}
        dateLabel={t('dialogs.task.fields.deadline.date')}
        timeLabel={t('dialogs.task.fields.deadline.time')}
        clearLabel={t('dialogs.task.fields.deadline.clear')}
        dateValue={form.deadlineDate}
        timeValue={form.deadlineTime}
        onChangeDate={(v) => update('deadlineDate', v)}
        onChangeTime={(v) => update('deadlineTime', v)}
        onClear={() => clearSlot('deadline')}
        dateRef={deadlineDateRef}
        editable={!loading}
      />

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
      </View>

      {canRecur && (
        <TaskRecurrenceSelector
          value={form.recurrence}
          onChange={(recurrence) => update('recurrence', recurrence)}
        />
      )}

      <RemindersEditor
        mode="task"
        value={form.reminders}
        onChange={(reminders) => update('reminders', reminders)}
      />

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
    </ScrollView>
  );
}

function DateTimeField({
  legend,
  hint,
  dateLabel,
  timeLabel,
  clearLabel,
  dateValue,
  timeValue,
  onChangeDate,
  onChangeTime,
  onClear,
  dateRef,
  editable,
}: {
  legend: string;
  hint: string;
  dateLabel: string;
  timeLabel: string;
  clearLabel: string;
  dateValue: string;
  timeValue: string;
  onChangeDate: (v: string) => void;
  onChangeTime: (v: string) => void;
  onClear: () => void;
  dateRef: RefObject<TextInput | null>;
  editable: boolean;
}) {
  const hasDate = dateValue.trim() !== '';
  return (
    <View style={styles.field}>
      <Text style={styles.legend}>{legend}</Text>
      {hint !== '' && (
        <Text style={styles.hint} accessibilityRole="text">
          {hint}
        </Text>
      )}
      <TextInput
        ref={dateRef}
        style={styles.input}
        value={dateValue}
        onChangeText={onChangeDate}
        placeholder="YYYY-MM-DD"
        // Qualify with the legend so the scheduled vs deadline date inputs (both
        // labelled "Date") are distinguishable when navigated control-by-control.
        accessibilityLabel={`${legend} – ${dateLabel}`}
        editable={editable}
        autoCapitalize="none"
        autoCorrect={false}
      />
      <TextInput
        style={styles.input}
        value={timeValue}
        onChangeText={onChangeTime}
        placeholder="HH:MM"
        accessibilityLabel={`${legend} – ${timeLabel}`}
        accessibilityState={{ disabled: !hasDate }}
        editable={editable && hasDate}
        autoCapitalize="none"
        autoCorrect={false}
      />
      {hasDate && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={clearLabel}
          onPress={onClear}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.ghostPressed]}
        >
          <Text style={styles.ghostButtonText}>{clearLabel}</Text>
        </Pressable>
      )}
    </View>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 20, gap: 18 },
  field: { gap: 6 },
  legend: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  hint: { fontSize: 13, color: '#5b6573' },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  multiline: { minHeight: 96 },
  buttons: { flexDirection: 'row', gap: 10, marginTop: 8 },
  button: {
    paddingVertical: 14,
    paddingHorizontal: 18,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  buttonPressed: { backgroundColor: '#1740a8' },
  buttonDisabled: { opacity: 0.5 },
  buttonText: { fontSize: 17, fontWeight: '700', color: '#ffffff' },
  ghostButton: {
    paddingVertical: 14,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
    alignItems: 'center',
  },
  ghostPressed: { backgroundColor: '#e4ebf5' },
  ghostButtonText: { fontSize: 17, fontWeight: '600', color: '#1d3a2f' },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
});
