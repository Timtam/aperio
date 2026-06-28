import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  type AccessibilityActionEvent,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { Task, TaskList, TaskPriority, TaskStatus } from '@aperio/shared';
import { prioritySuffix, statusI18nKey, statusMarker } from '@aperio/shared';

import { createTask, deleteTask, getTasks, updateTask } from '../api/client';
import { useListFocusManager } from '../a11y/useListFocusManager';
import { canStoreInProgress } from '../state/taskBehaviour';
import {
  applyTaskToggle,
  recomputeAncestors,
  setTaskStatusTo,
  statusAnnounce,
} from '../state/taskToggle';
import { useThemedStyles, type ThemeColors } from '../theme';

// In-editor subtask management — the RN port of the desktop TaskDialog's
// subtasks fieldset (edit mode). Mutations apply IMMEDIATELY (not staged with
// the parent form's Save), matching the desktop: the common case is "tick a
// subtask off mid-edit", which shouldn't be coupled to the parent's other
// unsaved field edits. Every mutation cascades to ancestors via the shared
// taskToggle helpers (so a reopened child re-opens a completed parent, etc.) and
// then refetches the owning list's snapshot. Screen-reader-first: each row is a
// button (press toggles done) carrying custom actions for in-progress / cancel /
// change-priority / delete — the mobile analogue of the desktop per-row context
// menu, since TalkBack/VoiceOver have no submenu idiom.

const PRIORITY_CYCLE: TaskPriority[] = ['low', 'medium', 'high'];

function nextPriority(p: TaskPriority): TaskPriority {
  const i = PRIORITY_CYCLE.indexOf(p);
  return PRIORITY_CYCLE[(i + 1) % PRIORITY_CYCLE.length];
}

/** dialogs.task.status.* key for a status (camelCased where the value isn't). */
const STATUS_LABEL_KEY: Record<TaskStatus, string> = {
  open: 'open',
  in_progress: 'inProgress',
  completed: 'completed',
  cancelled: 'cancelled',
};

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function SubtaskSection({
  parentTask,
  list,
  onChanged,
  onParentSync,
}: {
  /** The task whose children this section manages (the loaded editor task). */
  parentTask: Task;
  /** The parent's owning list (for the `supports_in_progress` capability + the
   *  per-list cascade/auto-date knobs). */
  list: TaskList | undefined;
  /** Bump the editor's data version so the underlying list refetches on close. */
  onChanged: () => void;
  /** Report the parent's fresh state after a mutation so the editor adopts a
   *  cascade-derived status (and doesn't revert it on Save). */
  onParentSync?: (parent: Task) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  // Full owning-list snapshot — the cascade planners walk it for ancestors, and
  // the children are filtered from it for display.
  const [all, setAll] = useState<Task[]>([]);
  const [newTitle, setNewTitle] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const subtasks = useMemo(
    () => all.filter((tk) => tk.parent_id === parentTask.id),
    [all, parentTask.id],
  );
  const canInProgress = canStoreInProgress(list);

  // Move SR focus to the new/sibling row after add/remove (RN won't on its own).
  const { registerRow, registerAdd, onAdd, onRemove } = useListFocusManager(subtasks.length);

  const announce = (message: string) => AccessibilityInfo.announceForAccessibility(message);

  const reload = useCallback(async () => {
    try {
      const snap = await getTasks(parentTask.list_id);
      setAll(snap);
      const freshParent = snap.find((tk) => tk.id === parentTask.id);
      if (freshParent) onParentSync?.(freshParent);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [parentTask.list_id, parentTask.id, onParentSync]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const add = useCallback(async () => {
    const title = newTitle.trim();
    if (!title || busy) return;
    setBusy(true);
    setError(null);
    try {
      await createTask({
        list_id: parentTask.list_id,
        title,
        description: null,
        status: 'open',
        priority: 'medium',
        effort: 'medium',
        scheduled_date: null,
        scheduled_time: null,
        deadline_date: null,
        deadline_time: null,
        recurrence: null,
        parent_id: parentTask.id,
        // Keep the subtask in its parent's section so it groups with it.
        section_id: parentTask.section_id,
        color_label: null,
        reminders: [],
        assignees: [],
        sound: null,
      });
      // A new open child can re-derive the parent (e.g. completed → in_progress)
      // — recompute ancestors against the post-create snapshot (coupling knob).
      await recomputeAncestors(parentTask.id, await getTasks(parentTask.list_id));
      setNewTitle('');
      onAdd();
      await reload();
      onChanged();
      announce(t('dialogs.task.subtasks.added', { title }));
    } catch (err) {
      setError(errorMessage(err));
      announce(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [newTitle, busy, parentTask, onAdd, reload, onChanged, t]);

  const toggle = useCallback(
    async (sub: Task) => {
      try {
        const next = await applyTaskToggle(sub, list, all);
        await reload();
        onChanged();
        if (next) announce(statusAnnounce(t, next, sub.title));
      } catch (err) {
        announce(errorMessage(err));
      }
    },
    [all, list, reload, onChanged, t],
  );

  const setStatus = useCallback(
    async (sub: Task, status: TaskStatus) => {
      try {
        await setTaskStatusTo(sub, status, list, all);
        await reload();
        onChanged();
        announce(
          t('mobile.subtaskStatusSet', {
            title: sub.title,
            status: t(`dialogs.task.status.${STATUS_LABEL_KEY[status]}`),
          }),
        );
      } catch (err) {
        announce(errorMessage(err));
      }
    },
    [all, list, reload, onChanged, t],
  );

  const cyclePriority = useCallback(
    async (sub: Task) => {
      const next = nextPriority(sub.priority);
      try {
        await updateTask({ ...sub, priority: next });
        await reload();
        onChanged();
        announce(t('mobile.priorityCycled', { priority: t(`dialogs.task.priority.${next}`) }));
      } catch (err) {
        announce(errorMessage(err));
      }
    },
    [reload, onChanged, t],
  );

  const remove = useCallback(
    async (sub: Task, index: number) => {
      try {
        await deleteTask(sub.id, sub.list_id);
        // Removing the last open child can mark the parent completed — recompute
        // ancestors against the post-deletion snapshot.
        await recomputeAncestors(parentTask.id, await getTasks(parentTask.list_id));
        onRemove(index);
        await reload();
        onChanged();
        announce(t('dialogs.task.deleted', { title: sub.title }));
      } catch (err) {
        announce(errorMessage(err));
      }
    },
    [parentTask.id, parentTask.list_id, onRemove, reload, onChanged, t],
  );

  const onAction = (sub: Task, index: number, event: AccessibilityActionEvent) => {
    switch (event.nativeEvent.actionName) {
      case 'toggle':
        void toggle(sub);
        break;
      case 'inprogress':
        void setStatus(sub, 'in_progress');
        break;
      case 'cancel':
        void setStatus(sub, 'cancelled');
        break;
      case 'priority':
        void cyclePriority(sub);
        break;
      case 'delete':
        void remove(sub, index);
        break;
    }
  };

  return (
    <View style={styles.field}>
      <Text style={styles.legend} accessibilityRole="header">
        {t('dialogs.task.subtasks.heading')}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {subtasks.length === 0 ? (
        <Text style={styles.empty} accessibilityRole="text">
          {t('dialogs.task.subtasks.empty')}
        </Text>
      ) : (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.task.subtasks.heading')}
          style={styles.list}
        >
          {subtasks.map((sub, i) => {
            const done = sub.status === 'completed';
            const actions = [
              { name: 'toggle', label: done ? t('mobile.reopen') : t('mobile.complete') },
            ];
            if (canInProgress && sub.status !== 'in_progress') {
              actions.push({ name: 'inprogress', label: t('dialogs.task.status.inProgress') });
            }
            if (sub.status !== 'cancelled') {
              actions.push({ name: 'cancel', label: t('dialogs.task.status.cancelled') });
            }
            actions.push({ name: 'priority', label: t('mobile.cyclePriority') });
            actions.push({ name: 'delete', label: t('mobile.delete') });
            return (
              <Pressable
                key={sub.id}
                ref={registerRow(i)}
                accessible
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.task.subtasks.rowLabel', {
                  title: sub.title,
                  state: t(statusI18nKey(sub.status)),
                  priority: prioritySuffix(t, sub.priority),
                })}
                accessibilityHint={t('mobile.subtaskRowHint')}
                accessibilityActions={actions}
                onAccessibilityAction={(e) => onAction(sub, i, e)}
                onPress={() => void toggle(sub)}
                style={({ pressed }) => [styles.row, pressed && styles.pressed]}
              >
                <Text style={styles.marker} importantForAccessibility="no">
                  {statusMarker(sub.status)}
                </Text>
                <Text style={styles.rowTitle} importantForAccessibility="no">
                  {sub.title}
                </Text>
              </Pressable>
            );
          })}
        </View>
      )}

      <View style={styles.addRow}>
        <TextInput
          style={styles.input}
          value={newTitle}
          onChangeText={setNewTitle}
          placeholder={t('dialogs.task.subtasks.placeholder')}
          accessibilityLabel={t('dialogs.task.subtasks.newAria')}
          editable={!busy}
          returnKeyType="done"
          onSubmitEditing={() => void add()}
        />
        <Pressable
          ref={registerAdd}
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.task.subtasks.addButton')}
          accessibilityState={{ disabled: busy || newTitle.trim() === '' }}
          disabled={busy || newTitle.trim() === ''}
          onPress={() => void add()}
          style={({ pressed }) => [
            styles.addButton,
            pressed && styles.pressed,
            (busy || newTitle.trim() === '') && styles.addButtonDisabled,
          ]}
        >
          <Text style={styles.addButtonText}>{t('dialogs.task.subtasks.addButton')}</Text>
        </Pressable>
      </View>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 8 },
    legend: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    empty: { fontSize: 14, color: c.textSecondary },
    list: { gap: 8 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    marker: { fontSize: 16, color: c.textSecondary, width: 20, textAlign: 'center' },
    rowTitle: { flex: 1, fontSize: 16, color: c.textPrimary },
    addRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
    input: {
      flex: 1,
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    addButtonDisabled: { opacity: 0.5 },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    pressed: { opacity: 0.7 },
    error: { fontSize: 14, fontWeight: '600', color: c.danger },
  });
