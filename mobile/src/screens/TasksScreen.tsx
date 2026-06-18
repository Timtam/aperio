import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityActionEvent,
  AccessibilityInfo,
  ActivityIndicator,
  Alert,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';

import type { Entry, Task } from '@aperio/shared';
import {
  assigneeSuffix,
  buildEntries,
  DEFERRED_GROUP_ID,
  DONE_GROUP_ID,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskProgressSuffix,
} from '@aperio/shared';

import { deleteTask, updateTask } from '../api/client';
import { useCurrentDayKey } from '../hooks/useCurrentDayKey';
import { describeDue } from '../intl/describeDue';
import { useTaskStore } from '../state/taskStoreContext';
import { useTasks } from '../state/useTasks';
import type { RootStackScreenProps } from '../navigation/types';

// The grouped tasks tree — a faithful port of the desktop TaskView, adapted to
// the RN per-element + custom-actions a11y idiom (no aria-tree / roving focus).
// Grouping + labels are NOT reimplemented: buildEntries + the status/label
// helpers come from @aperio/shared, exactly as the desktop calls them. Group
// headers (Backlog / per-list / per-section / Upcoming / Done) are collapsible
// buttons; task rows carry the rich shared label + complete/edit/delete (and a
// subtask-collapse action for parents). The rich editor is sub-4, section/list
// management sub-5.

// Only Done + Upcoming collapse state persists (per the desktop); per-list,
// per-section and subtask twisties stay session-local.
const DONE_COLLAPSE_KEY = 'aperio.tasks.doneCollapsed';
const DEFERRED_COLLAPSE_KEY = 'aperio.tasks.deferredCollapsed';

export default function TasksScreen({
  navigation,
}: RootStackScreenProps<'Tasks'>) {
  const { t, i18n } = useTranslation();
  const { tasks, loading, taskListById } = useTasks();
  const {
    selectedTaskListIds,
    taskLists,
    sectionsByList,
    loadSections,
    colorLabels,
    invalidateData,
  } = useTaskStore();

  // The shared helpers + buildEntries take a plain (key, vars) => string; adapt
  // i18next's t (whose overload union isn't directly assignable).
  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars) as string,
    [t],
  );

  // Localized, time-free date formatter — Intl approximates the desktop's
  // date-fns 'PP' (no date-fns dependency on mobile).
  const formatDate = useMemo(() => {
    const f = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
    return (iso: string) => f.format(new Date(iso));
  }, [i18n.language]);

  const today = useCurrentDayKey();

  // Collapsed group/subtask ids. Done + Upcoming start collapsed (desktop
  // default); hydrated from storage, which can only EXPAND them (explicit
  // 'false') so a not-yet-loaded read never flickers them open.
  const [collapsed, setCollapsed] = useState<Set<string>>(
    () => new Set([DONE_GROUP_ID, DEFERRED_GROUP_ID]),
  );
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const [doneRaw, deferredRaw] = await Promise.all([
        AsyncStorage.getItem(DONE_COLLAPSE_KEY),
        AsyncStorage.getItem(DEFERRED_COLLAPSE_KEY),
      ]);
      if (cancelled) return;
      if (doneRaw === 'false' || deferredRaw === 'false') {
        setCollapsed((prev) => {
          const next = new Set(prev);
          if (doneRaw === 'false') next.delete(DONE_GROUP_ID);
          if (deferredRaw === 'false') next.delete(DEFERRED_GROUP_ID);
          return next;
        });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Spoken feedback (plain announce, not a live region).
  const announce = useCallback((message: string) => {
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  // Managed screen-reader focus: rows + headers register their node handles so
  // focus can be restored after a mutation refetch (rows) or a collapse
  // (headers stay mounted). pendingEmptyFocus lands on the empty-state message
  // when a delete empties the list.
  const rowTags = useRef<Record<string, number | null>>({});
  const headerTags = useRef<Record<string, number | null>>({});
  const emptyRef = useRef<Text>(null);
  const pendingFocusId = useRef<string | null>(null);
  const pendingEmptyFocus = useRef(false);

  // Load sections for every list that has tasks so buildEntries can group by
  // section. Mirrors TaskView's sections-loading effect; lazy + sticky.
  const listIdsWithTasks = useMemo(
    () => Array.from(new Set(tasks.map((task) => task.list_id))),
    [tasks],
  );
  useEffect(() => {
    listIdsWithTasks.forEach((listId) => {
      if (!(listId in sectionsByList)) void loadSections(listId);
    });
  }, [listIdsWithTasks, sectionsByList, loadSections]);

  const entries = useMemo(
    () => buildEntries(tasks, taskListById, tr, collapsed, sectionsByList, today).entries,
    [tasks, taskListById, tr, collapsed, sectionsByList, today],
  );

  // Restore focus to a sibling row after a mutation refetch remounts the list.
  useEffect(() => {
    if (pendingFocusId.current != null) {
      const id = pendingFocusId.current;
      pendingFocusId.current = null;
      pendingEmptyFocus.current = false;
      const tag = rowTags.current[id] ?? headerTags.current[id];
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
      return;
    }
    if (pendingEmptyFocus.current) {
      pendingEmptyFocus.current = false;
      const tag = findNodeHandle(emptyRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    }
  }, [entries]);

  const targetListId = [...selectedTaskListIds][0] ?? taskLists[0]?.id ?? null;

  const openEditor = useCallback(
    (task: Task) =>
      navigation.navigate('TaskEditor', { taskId: task.id, listId: task.list_id }),
    [navigation],
  );

  const newTask = useCallback(() => {
    if (targetListId == null) {
      navigation.navigate('Lists');
      return;
    }
    navigation.navigate('TaskEditor', { taskId: null, listId: targetListId });
  }, [navigation, targetListId]);

  const toggleDone = useCallback(
    async (task: Task) => {
      const done = task.status === 'completed';
      // Keep SR focus sensible after the refetch remounts the list: reopening
      // makes the task visible again (focus it); completing slips it into the
      // (collapsed) Done group, so focus a surviving sibling — or the Done
      // header itself when it was the only visible task.
      pendingFocusId.current = done
        ? task.id
        : (focusTargetAfterRemoving(entries, task.id) ?? DONE_GROUP_ID);
      try {
        await updateTask({
          ...task,
          status: done ? 'open' : 'completed',
          completed_at: done ? null : new Date().toISOString(),
        });
        invalidateData();
        announce(
          done
            ? t('mobile.reopened', { title: task.title })
            : t('mobile.completed', { title: task.title }),
        );
      } catch (err) {
        pendingFocusId.current = null;
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, entries, invalidateData, t],
  );

  const removeTask = useCallback(
    (task: Task) => {
      const performDelete = async () => {
        // Land focus on a surviving sibling (excluding the deleted task's own
        // subtree, which cascades away with it), or the empty-state message.
        const siblingId = focusTargetAfterRemoving(entries, task.id);
        if (siblingId) {
          pendingFocusId.current = siblingId;
        } else {
          pendingEmptyFocus.current = true;
        }
        try {
          await deleteTask(task.id, task.list_id);
          invalidateData();
          announce(t('mobile.deleted', { title: task.title }));
        } catch (err) {
          pendingFocusId.current = null;
          pendingEmptyFocus.current = false;
          announce(t('mobile.error', { message: errorMessage(err) }));
        }
      };
      // Confirm the irreversible delete — matches the desktop ConfirmDialog.
      // Alert is screen-reader-accessible and moves focus to itself.
      Alert.alert(
        t('dialogs.confirm.deleteTaskTitle'),
        t('dialogs.confirm.deleteTaskMessage', { title: task.title }),
        [
          { text: t('dialogs.confirm.cancel'), style: 'cancel' },
          {
            text: t('mobile.delete'),
            style: 'destructive',
            onPress: () => void performDelete(),
          },
        ],
      );
    },
    [announce, entries, invalidateData, t],
  );

  // Toggle a group header (sentinel/synthetic id) or a subtask parent (task id).
  const toggleCollapsed = useCallback(
    (id: string, title: string) => {
      const willExpand = collapsed.has(id);
      setCollapsed((prev) => {
        const next = new Set(prev);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        if (id === DONE_GROUP_ID) {
          void AsyncStorage.setItem(DONE_COLLAPSE_KEY, String(next.has(id)));
        } else if (id === DEFERRED_GROUP_ID) {
          void AsyncStorage.setItem(DEFERRED_COLLAPSE_KEY, String(next.has(id)));
        }
        return next;
      });
      announce(
        willExpand
          ? t('mobile.groupExpanded', { name: title })
          : t('mobile.groupCollapsed', { name: title }),
      );
      // The toggled header/row stays mounted (stable key) — only its children
      // unmount — so TalkBack/VoiceOver retain focus on it. We deliberately do
      // NOT force focus back: refocusing the already-focused node re-speaks its
      // title on iOS and would step on the expanded/collapsed announcement.
    },
    [announce, collapsed, t],
  );

  const onAction = useCallback(
    (entry: Entry, event: AccessibilityActionEvent) => {
      const task = entry.task;
      switch (event.nativeEvent.actionName) {
        case 'toggle':
          void toggleDone(task);
          break;
        case 'edit':
          openEditor(task);
          break;
        case 'delete':
          void removeTask(task);
          break;
        case 'toggleCollapse':
          toggleCollapsed(task.id, task.title);
          break;
      }
    },
    [openEditor, removeTask, toggleCollapsed, toggleDone],
  );

  const taskLabel = (task: Task): string => {
    const base = t('views.tasks.optionLabel', {
      title: task.title,
      state: t(statusI18nKey(task.status)),
      priority: prioritySuffix(tr, task.priority),
      progress: subtaskProgressSuffix(tr, task.id, tasks),
      due: describeDue(task, tr, today, formatDate),
      assignee: assigneeSuffix(tr, task.assignees),
    });
    // A bound colour is meaningless to a screen reader as a colour, so announce
    // its label NAME instead (resolved from the palette).
    const colour = task.color_label
      ? colorLabels.find((l) => l.id === task.color_label)?.name
      : undefined;
    return colour ? `${base}${t('mobile.colorLabelSuffix', { name: colour })}` : base;
  };

  const renderHeader = (entry: Entry) => {
    const id = entry.task.id;
    const isCollapsed = collapsed.has(id);
    return (
      <Pressable
        key={id}
        ref={(node) => {
          headerTags.current[id] = node ? findNodeHandle(node) : null;
        }}
        accessible
        accessibilityRole="button"
        accessibilityLabel={entry.task.title}
        accessibilityHint={t('mobile.groupHeaderHint')}
        accessibilityState={entry.hasChildren ? { expanded: !isCollapsed } : undefined}
        onPress={() => toggleCollapsed(id, entry.task.title)}
        style={({ pressed }) => [
          styles.header,
          { paddingLeft: 16 + entry.depth * 16 },
          pressed && styles.rowPressed,
        ]}
      >
        <Text style={styles.twisty} importantForAccessibility="no">
          {isCollapsed ? '▸' : '▾'}
        </Text>
        <Text style={styles.headerTitle} importantForAccessibility="no">
          {entry.task.title}
        </Text>
      </Pressable>
    );
  };

  const renderTask = (entry: Entry) => {
    const task = entry.task;
    const done = task.status === 'completed';
    // The task's bound colour (for sighted users — a coloured dot on the row).
    // Screen-reader users get the label name in taskLabel() instead.
    const taskHex = task.color_label
      ? colorLabels.find((l) => l.id === task.color_label)?.hex
      : undefined;
    const actions = [
      { name: 'toggle', label: done ? t('mobile.reopen') : t('mobile.complete') },
      { name: 'edit', label: t('mobile.rename') },
      { name: 'delete', label: t('mobile.delete') },
    ];
    if (entry.hasChildren) {
      actions.push({
        name: 'toggleCollapse',
        label: collapsed.has(task.id)
          ? t('views.tasks.expand', { title: task.title })
          : t('views.tasks.collapse', { title: task.title }),
      });
    }
    return (
      <Pressable
        key={task.id}
        ref={(node) => {
          rowTags.current[task.id] = node ? findNodeHandle(node) : null;
        }}
        accessible
        accessibilityRole="button"
        accessibilityLabel={taskLabel(task)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityState={
          entry.hasChildren ? { expanded: !collapsed.has(task.id) } : undefined
        }
        accessibilityActions={actions}
        onAccessibilityAction={(event) => onAction(entry, event)}
        onPress={() => openEditor(task)}
        style={({ pressed }) => [
          styles.task,
          { marginLeft: entry.depth * 16 },
          pressed && styles.rowPressed,
        ]}
      >
        <Text style={styles.taskCheck} importantForAccessibility="no">
          {statusMarker(task.status)}
        </Text>
        {taskHex != null && (
          <View
            accessible={false}
            importantForAccessibility="no"
            style={[styles.colorDot, { backgroundColor: taskHex }]}
          />
        )}
        <View style={styles.taskBody}>
          <Text
            style={[styles.taskTitle, done && styles.taskTitleDone]}
            importantForAccessibility="no"
          >
            {task.title}
          </Text>
          <Text style={styles.taskMeta} importantForAccessibility="no">
            {describeDue(task, tr, today, formatDate)}
          </Text>
        </View>
      </Pressable>
    );
  };

  return (
    <View style={styles.screen}>
      <View style={styles.toolbar}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.newTaskLabel')}
          onPress={newTask}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('mobile.add')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.listsButtonLabel')}
          onPress={() => navigation.navigate('Lists')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.rowPressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.listsButtonLabel')}</Text>
        </Pressable>
        {/* Calendar / Contacts / Settings are now bottom tabs, not toolbar
            buttons — the Tasks toolbar keeps only its own actions (Add + Lists). */}
      </View>

      {loading ? (
        <View
          style={styles.center}
          accessible
          accessibilityRole="text"
          accessibilityLabel={t('mobile.loadingLabel')}
        >
          <ActivityIndicator />
          <Text style={styles.muted}>{t('mobile.loading')}</Text>
        </View>
      ) : entries.length === 0 ? (
        <Text ref={emptyRef} accessibilityRole="text" style={styles.muted}>
          {t('mobile.empty')}
        </Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {entries
            .filter((entry) => !entry.hidden)
            .map((entry) => (entry.group ? renderHeader(entry) : renderTask(entry)))}
        </ScrollView>
      )}
    </View>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * The id of the next (or previous) VISIBLE task row to land screen-reader focus
 * on after `taskId` leaves the visible list (deleted, or completed into a
 * collapsed group). Excludes `taskId` AND its whole subtree — a parent's
 * subtasks move/cascade away with it, so they're never valid focus targets.
 * Returns `null` when no other visible task remains.
 */
function focusTargetAfterRemoving(entries: Entry[], taskId: string): string | null {
  const excluded = new Set<string>([taskId]);
  const idx = entries.findIndex((e) => !e.group && e.task.id === taskId);
  if (idx >= 0) {
    const depth = entries[idx].depth;
    for (let i = idx + 1; i < entries.length; i += 1) {
      if (entries[i].depth <= depth) break;
      if (!entries[i].group) excluded.add(entries[i].task.id);
    }
  }
  const visible = entries.filter((e) => !e.hidden && !e.group);
  const pos = visible.findIndex((e) => e.task.id === taskId);
  if (pos < 0) return null;
  const after = visible.slice(pos + 1).find((e) => !excluded.has(e.task.id));
  if (after) return after.task.id;
  for (let i = pos - 1; i >= 0; i -= 1) {
    if (!excluded.has(visible[i].task.id)) return visible[i].task.id;
  }
  return null;
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  toolbar: {
    flexDirection: 'row',
    gap: 10,
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  list: { gap: 8, padding: 16 },
  button: {
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  buttonPressed: { backgroundColor: '#1740a8' },
  buttonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  ghostButton: {
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
    alignItems: 'center',
  },
  ghostButtonText: { fontSize: 16, fontWeight: '600', color: '#1d3a2f' },
  center: { alignItems: 'center', gap: 8, paddingVertical: 24 },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 12,
    paddingRight: 16,
    borderRadius: 8,
    backgroundColor: '#eef2f8',
  },
  twisty: { fontSize: 16, width: 18, textAlign: 'center', color: '#2b3240' },
  headerTitle: { flex: 1, fontSize: 16, fontWeight: '700', color: '#10131a' },
  task: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 14,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowPressed: { backgroundColor: '#e4ebf5' },
  taskCheck: { fontSize: 20, width: 26, textAlign: 'center', color: '#10131a' },
  // A small colour dot for the task's bound colour label (sighted users).
  colorDot: {
    width: 12,
    height: 12,
    borderRadius: 6,
    borderWidth: 1,
    borderColor: 'rgba(0,0,0,0.18)',
  },
  taskBody: { flex: 1 },
  taskTitle: { fontSize: 18, color: '#10131a' },
  taskTitleDone: { textDecorationLine: 'line-through', color: '#5b6573' },
  taskMeta: { fontSize: 14, color: '#5b6573', marginTop: 2 },
});
