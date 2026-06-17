import { useCallback, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityActionEvent,
  AccessibilityInfo,
  ActivityIndicator,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import type { Task } from '@aperio/shared';

import { deleteTask, updateTask } from '../api/client';
import { useTaskStore } from '../state/taskStoreContext';
import { useTasks } from '../state/useTasks';
import type { RootStackScreenProps } from '../navigation/types';

// The tasks list — minimal-but-real foundation (sub-3 swaps the flat list for
// the grouped Backlog / Zukünftig / per-list / per-section / Done tree built
// from @aperio/shared's buildEntries, and the shared status/label helpers).
// Mutations are real: toggle + delete go through the api-client then bump
// dataVersion so useTasks refetches — the desktop's invalidate loop.

export default function TasksScreen({
  navigation,
}: RootStackScreenProps<'Tasks'>) {
  const { t } = useTranslation();
  const { tasks, loading } = useTasks();
  const { selectedTaskListIds, taskLists, invalidateData } = useTaskStore();

  // Spoken feedback. The status line is plain text (NOT a live region) so it's
  // shown visually without TalkBack double-announcing on every re-render.
  const announce = useCallback((message: string) => {
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  // Screen-reader focus management around mutations: a mutation bumps
  // dataVersion, useTasks refetches, and the list remounts — dropping SR focus.
  // We capture where focus should land before the await and restore it once
  // the refetched list re-renders. For toggle/delete-with-sibling that's a row;
  // when a delete empties the list there's no row, so we land on the
  // empty-state message instead of orphaning focus at the top of the screen.
  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);
  const emptyRef = useRef<Text>(null);
  const pendingEmptyFocus = useRef(false);

  useEffect(() => {
    if (pendingFocusId.current != null) {
      const id = pendingFocusId.current;
      pendingFocusId.current = null;
      pendingEmptyFocus.current = false;
      const tag = rowTags.current[id];
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
      return;
    }
    if (pendingEmptyFocus.current) {
      pendingEmptyFocus.current = false;
      const tag = findNodeHandle(emptyRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    }
  }, [tasks]);

  const targetListId =
    [...selectedTaskListIds][0] ?? taskLists[0]?.id ?? null;

  const openEditor = useCallback(
    (task: Task) =>
      navigation.navigate('TaskEditor', {
        taskId: task.id,
        listId: task.list_id,
      }),
    [navigation],
  );

  const newTask = useCallback(() => {
    if (targetListId == null) {
      // No list to add to yet — send the user to create one first.
      navigation.navigate('Lists');
      return;
    }
    navigation.navigate('TaskEditor', { taskId: null, listId: targetListId });
  }, [navigation, targetListId]);

  const toggleDone = useCallback(
    async (task: Task) => {
      const done = task.status === 'completed';
      // Keep SR focus on this row once the refetch remounts the list.
      pendingFocusId.current = task.id;
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
    [announce, invalidateData, t],
  );

  const removeTask = useCallback(
    async (task: Task) => {
      const idx = tasks.findIndex((x) => x.id === task.id);
      const sibling = tasks[idx + 1] ?? tasks[idx - 1];
      if (sibling) {
        pendingFocusId.current = sibling.id;
      } else {
        // Deleting the last task — land focus on the empty-state message.
        pendingEmptyFocus.current = true;
      }
      try {
        await deleteTask(task.id);
        invalidateData();
        announce(t('mobile.deleted', { title: task.title }));
      } catch (err) {
        pendingFocusId.current = null;
        pendingEmptyFocus.current = false;
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, invalidateData, t, tasks],
  );

  const onAction = useCallback(
    (task: Task, event: AccessibilityActionEvent) => {
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
      }
    },
    [openEditor, removeTask, toggleDone],
  );

  const taskLabel = (task: Task): string => {
    const state =
      task.status === 'completed'
        ? t('mobile.statusDone')
        : t('mobile.statusOpen');
    const due =
      task.scheduled_date != null
        ? `, ${t('mobile.due', { date: task.scheduled_date })}`
        : '';
    return `${task.title}, ${state}${due}`;
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
      ) : tasks.length === 0 ? (
        <Text ref={emptyRef} accessibilityRole="text" style={styles.muted}>
          {t('mobile.empty')}
        </Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {tasks.map((task) => {
            const done = task.status === 'completed';
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
                accessibilityActions={[
                  {
                    name: 'toggle',
                    label: done ? t('mobile.reopen') : t('mobile.complete'),
                  },
                  { name: 'edit', label: t('mobile.rename') },
                  { name: 'delete', label: t('mobile.delete') },
                ]}
                onAccessibilityAction={(event) => onAction(task, event)}
                onPress={() => openEditor(task)}
                style={({ pressed }) => [styles.task, pressed && styles.rowPressed]}
              >
                <Text style={styles.taskCheck}>{done ? '✓' : '○'}</Text>
                <View style={styles.taskBody}>
                  <Text style={[styles.taskTitle, done && styles.taskTitleDone]}>
                    {task.title}
                  </Text>
                  {task.scheduled_date != null && (
                    <Text style={styles.taskDue}>
                      {t('mobile.due', { date: task.scheduled_date })}
                    </Text>
                  )}
                </View>
              </Pressable>
            );
          })}
        </ScrollView>
      )}
    </View>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  toolbar: {
    flexDirection: 'row',
    gap: 10,
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  list: { gap: 12, padding: 16 },
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
  taskCheck: { fontSize: 22, width: 26, textAlign: 'center', color: '#10131a' },
  taskBody: { flex: 1 },
  taskTitle: { fontSize: 18, color: '#10131a' },
  taskTitleDone: { textDecorationLine: 'line-through', color: '#5b6573' },
  taskDue: { fontSize: 14, color: '#5b6573', marginTop: 2 },
});
