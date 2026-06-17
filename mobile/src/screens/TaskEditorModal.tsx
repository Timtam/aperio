import { useCallback, useEffect, useRef, useState } from 'react';
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

import type { Task } from '@aperio/shared';

import { createTask, getTaskById, updateTask } from '../api/client';
import { useTaskStore } from '../state/taskStoreContext';
import type { RootStackScreenProps } from '../navigation/types';

// The task editor — a minimal-but-real stub (sub-4 fleshes it into the full
// editor: status/priority pickers, sections, reminders, recurrence,
// assignees, the plan-status cascade). It proves the create + edit round-trip
// and the desktop's "close() always bumps dataVersion" wiring: the modal bumps
// on unmount, so ANY dismissal (Save, Cancel, swipe-down, header back) makes
// the tasks list refetch.

export default function TaskEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'TaskEditor'>) {
  const { t } = useTranslation();
  const { taskId, listId } = route.params;
  const { invalidateData } = useTaskStore();

  const [title, setTitle] = useState('');
  const [due, setDue] = useState('');
  const [loaded, setLoaded] = useState<Task | null>(null);
  const [loading, setLoading] = useState(taskId != null);
  const [error, setError] = useState<string | null>(null);

  const titleRef = useRef<TextInput | null>(null);

  // Header title (announced by the screen reader on present) reflects the mode.
  useEffect(() => {
    navigation.setOptions({
      title:
        taskId == null ? t('mobile.newTaskLabel') : t('mobile.editTaskLabel'),
    });
  }, [navigation, t, taskId]);

  // Faithful to the desktop's DialogState.close(): every dismissal bumps the
  // data version so the tasks list refetches. Firing on unmount covers Save,
  // Cancel, the header back button and the swipe-down gesture alike.
  useEffect(() => {
    return () => invalidateData();
  }, [invalidateData]);

  // Edit mode: load the existing task and prefill. Create mode starts blank.
  useEffect(() => {
    if (taskId == null) return;
    let cancelled = false;
    void (async () => {
      try {
        const task = await getTaskById(taskId);
        if (cancelled) return;
        if (task == null) {
          // The row vanished (or the store failed to read it) — surface it
          // rather than presenting a blank form that silently saves nothing.
          setError(t('mobile.taskMissing'));
          AccessibilityInfo.announceForAccessibility(t('mobile.taskMissing'));
          return;
        }
        setLoaded(task);
        setTitle(task.title);
        setDue(task.scheduled_date ?? '');
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [taskId, t]);

  // Move screen-reader focus into the title field once content is ready — a
  // modal must drive SR focus explicitly or TalkBack lingers on the trigger.
  useEffect(() => {
    if (loading) return;
    const tag = findNodeHandle(titleRef.current);
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [loading]);

  const save = useCallback(async () => {
    const trimmed = title.trim();
    if (trimmed.length === 0) return;
    const scheduled = due.trim().length > 0 ? due.trim() : null;
    setError(null);
    try {
      if (taskId == null) {
        await createTask({
          list_id: listId,
          title: trimmed,
          description: null,
          status: 'open',
          priority: 'medium',
          scheduled_date: scheduled,
          scheduled_time: null,
          deadline_date: null,
          deadline_time: null,
          recurrence: null,
          parent_id: null,
          section_id: null,
          color_label: null,
          reminders: [],
          assignees: [],
          sound: null,
        });
        AccessibilityInfo.announceForAccessibility(
          t('mobile.added', { title: trimmed }),
        );
      } else if (loaded != null) {
        await updateTask({ ...loaded, title: trimmed, scheduled_date: scheduled });
        AccessibilityInfo.announceForAccessibility(
          t('mobile.saved', { title: trimmed }),
        );
      } else {
        // Edit target failed to load — the error banner already explains it;
        // don't dismiss as if we saved.
        return;
      }
      navigation.goBack();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      AccessibilityInfo.announceForAccessibility(
        t('mobile.error', { message }),
      );
    }
  }, [due, listId, loaded, navigation, t, taskId, title]);

  // In edit mode, block saving until the task has loaded (and keep it blocked
  // if the load failed) so a tap can't silently no-op.
  const blocked = taskId != null && loaded == null;

  return (
    <View style={styles.screen} accessibilityViewIsModal>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text">
          {error}
        </Text>
      )}
      <TextInput
        ref={titleRef}
        style={styles.input}
        value={title}
        onChangeText={setTitle}
        placeholder={t('mobile.newTaskPlaceholder')}
        accessibilityLabel={t('mobile.taskTitleLabel')}
        editable={!loading}
        returnKeyType="next"
      />
      <TextInput
        style={styles.input}
        value={due}
        onChangeText={setDue}
        placeholder={t('mobile.dueDatePlaceholder')}
        accessibilityLabel={t('mobile.dueDateLabel')}
        editable={!loading}
        autoCapitalize="none"
        autoCorrect={false}
        returnKeyType="done"
        onSubmitEditing={() => void save()}
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
    </View>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff', padding: 20, gap: 14 },
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
  buttons: { flexDirection: 'row', gap: 10 },
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
