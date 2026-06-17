import { StatusBar } from 'expo-status-bar';
import { useCallback, useEffect, useRef, useState } from 'react';
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
  TextInput,
  View,
} from 'react-native';

import CalFfi from './modules/cal-ffi';
import type { TaskView } from './modules/cal-ffi';

// Aperio mobile — minimal accessibility-first tasks screen (M1, offline).
//
// Persistence runs through the shared Rust core: `CalFfi` is the UniFFI
// `LocalStore` over an app-private SQLite file (migrated by the same
// `aperio-db` runner the desktop uses). The UI is rebuilt per platform; the
// strings are NOT — every label comes from i18next, reusing the desktop's
// shared translation files (`@aperio/locales`), with mobile-only wording under
// the `mobile.*` key.
//
// Accessibility (validated on-device in the pilot): semantic headings/labels,
// a `list` container, custom accessibility ACTIONS per task (complete/reopen,
// rename, delete), spoken feedback via `announceForAccessibility`, and managed
// screen-reader focus around the rename edit mode.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function App() {
  const { t } = useTranslation();

  const [listId, setListId] = useState<string | null>(null);
  const [tasks, setTasks] = useState<TaskView[]>([]);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState('');
  const [error, setError] = useState<string | null>(null);

  const [newTitle, setNewTitle] = useState('');
  const [newDue, setNewDue] = useState('');

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');

  // Screen-reader focus management: `autoFocus` raises the keyboard but does
  // not reliably move TalkBack focus, so we drive it explicitly.
  const editInputRef = useRef<TextInput | null>(null);
  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  // Spoken feedback. The status line below is plain (not a live region) so it
  // is shown visually without TalkBack double-announcing on every re-render.
  const announce = useCallback((message: string) => {
    setStatus(message);
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const refresh = useCallback(async (id: string) => {
    setTasks(await CalFfi.tasks(id));
  }, []);

  // Move SR focus into the rename field when edit mode opens, and back to the
  // edited row when it closes (the row's node is reused thanks to a stable key).
  useEffect(() => {
    if (editingId != null) {
      const tag = findNodeHandle(editInputRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    } else if (pendingFocusId.current != null) {
      const id = pendingFocusId.current;
      pendingFocusId.current = null;
      const tag = rowTags.current[id];
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    }
  }, [editingId]);

  // Bootstrap once: the native store opens lazily on first call; ensure a list
  // exists to hold tasks, then load it. (`t` is stable for the session — there
  // is no in-app language switch yet — so this does not re-run.)
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const lists = await CalFfi.taskLists();
        const list = lists[0] ?? (await CalFfi.createTaskList(t('mobile.defaultListName')));
        if (cancelled) return;
        setListId(list.id);
        const loaded = await CalFfi.tasks(list.id);
        if (!cancelled) setTasks(loaded);
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [t]);

  const addTask = useCallback(async () => {
    const title = newTitle.trim();
    if (listId == null || title.length === 0) return;
    const due = newDue.trim();
    setError(null);
    try {
      await CalFfi.createTask(listId, title, null, due.length > 0 ? due : null);
      setNewTitle('');
      setNewDue('');
      await refresh(listId);
      announce(t('mobile.added', { title }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.addFailed', { message }));
    }
  }, [announce, listId, newDue, newTitle, refresh, t]);

  const toggleDone = useCallback(
    async (task: TaskView) => {
      if (listId == null) return;
      try {
        await CalFfi.setTaskDone(task.id, !task.done);
        await refresh(listId);
        announce(
          task.done
            ? t('mobile.reopened', { title: task.title })
            : t('mobile.completed', { title: task.title }),
        );
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, listId, refresh, t],
  );

  const removeTask = useCallback(
    async (task: TaskView) => {
      if (listId == null) return;
      try {
        await CalFfi.deleteTask(task.id);
        await refresh(listId);
        announce(t('mobile.deleted', { title: task.title }));
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, listId, refresh, t],
  );

  const startRename = useCallback((task: TaskView) => {
    setEditingId(task.id);
    setEditTitle(task.title);
  }, []);

  const cancelRename = useCallback(() => {
    setEditingId(null);
    setEditTitle('');
  }, []);

  const saveRename = useCallback(async () => {
    const title = editTitle.trim();
    if (listId == null || editingId == null || title.length === 0) {
      pendingFocusId.current = editingId;
      cancelRename();
      return;
    }
    const id = editingId;
    try {
      await CalFfi.renameTask(id, title);
      pendingFocusId.current = id;
      cancelRename();
      await refresh(listId);
    } catch (err) {
      announce(t('mobile.error', { message: errorMessage(err) }));
    }
  }, [announce, cancelRename, editTitle, editingId, listId, refresh, t]);

  const onAction = useCallback(
    (task: TaskView, event: AccessibilityActionEvent) => {
      switch (event.nativeEvent.actionName) {
        case 'toggle':
          void toggleDone(task);
          break;
        case 'rename':
          startRename(task);
          break;
        case 'delete':
          void removeTask(task);
          break;
      }
    },
    [removeTask, startRename, toggleDone],
  );

  const taskLabel = (task: TaskView): string => {
    const state = task.done ? t('mobile.statusDone') : t('mobile.statusOpen');
    const due =
      task.scheduledDate != null ? `, ${t('mobile.due', { date: task.scheduledDate })}` : '';
    return `${task.title}, ${state}${due}`;
  };

  return (
    <View style={styles.screen}>
      <StatusBar style="auto" />
      <ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
        <Text accessibilityRole="header" style={styles.heading}>
          {t('mobile.tasksHeading')}
        </Text>

        <Text style={styles.intro}>{t('mobile.intro')}</Text>

        <View style={styles.form}>
          <TextInput
            style={styles.input}
            value={newTitle}
            onChangeText={setNewTitle}
            placeholder={t('mobile.newTaskPlaceholder')}
            accessibilityLabel={t('mobile.newTaskLabel')}
            returnKeyType="done"
            onSubmitEditing={() => void addTask()}
          />
          <TextInput
            style={styles.input}
            value={newDue}
            onChangeText={setNewDue}
            placeholder={t('mobile.dueDatePlaceholder')}
            accessibilityLabel={t('mobile.dueDateLabel')}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="done"
            onSubmitEditing={() => void addTask()}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('mobile.addButtonLabel')}
            onPress={() => void addTask()}
            style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
          >
            <Text style={styles.buttonText}>{t('mobile.add')}</Text>
          </Pressable>
        </View>

        {status.length > 0 && (
          <Text style={styles.status} accessibilityRole="text">
            {status}
          </Text>
        )}
        {error != null && (
          <Text style={styles.error} accessibilityRole="text">
            {error}
          </Text>
        )}

        {loading ? (
          <View
            style={styles.center}
            accessibilityRole="text"
            accessibilityLabel={t('mobile.loadingLabel')}
          >
            <ActivityIndicator />
            <Text style={styles.muted}>{t('mobile.loading')}</Text>
          </View>
        ) : tasks.length === 0 ? (
          <Text style={styles.muted}>{t('mobile.empty')}</Text>
        ) : (
          <View accessibilityRole="list" style={styles.list}>
            {tasks.map((task) =>
              editingId === task.id ? (
                <View key={task.id} style={styles.editRow}>
                  <TextInput
                    ref={editInputRef}
                    style={styles.input}
                    value={editTitle}
                    onChangeText={setEditTitle}
                    accessibilityLabel={t('mobile.renameLabel')}
                    autoFocus
                    returnKeyType="done"
                    onSubmitEditing={() => void saveRename()}
                  />
                  <View style={styles.editButtons}>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={t('mobile.renameSaveLabel')}
                      onPress={() => void saveRename()}
                      style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
                    >
                      <Text style={styles.buttonText}>{t('mobile.save')}</Text>
                    </Pressable>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={t('mobile.renameCancelLabel')}
                      onPress={cancelRename}
                      style={({ pressed }) => [styles.ghostButton, pressed && styles.taskPressed]}
                    >
                      <Text style={styles.ghostButtonText}>{t('mobile.cancel')}</Text>
                    </Pressable>
                  </View>
                </View>
              ) : (
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
                    { name: 'toggle', label: task.done ? t('mobile.reopen') : t('mobile.complete') },
                    { name: 'rename', label: t('mobile.rename') },
                    { name: 'delete', label: t('mobile.delete') },
                  ]}
                  onAccessibilityAction={(event) => onAction(task, event)}
                  onPress={() => void toggleDone(task)}
                  style={({ pressed }) => [styles.task, pressed && styles.taskPressed]}
                >
                  <Text style={styles.taskCheck}>{task.done ? '✓' : '○'}</Text>
                  <View style={styles.taskBody}>
                    <Text style={[styles.taskTitle, task.done && styles.taskTitleDone]}>
                      {task.title}
                    </Text>
                    {task.scheduledDate != null && (
                      <Text style={styles.taskDue}>{t('mobile.due', { date: task.scheduledDate })}</Text>
                    )}
                  </View>
                </Pressable>
              ),
            )}
          </View>
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: '#ffffff',
  },
  content: {
    paddingTop: 72,
    paddingHorizontal: 20,
    paddingBottom: 40,
    gap: 16,
  },
  heading: {
    fontSize: 26,
    fontWeight: '700',
    color: '#10131a',
  },
  intro: {
    fontSize: 16,
    lineHeight: 22,
    color: '#2b3240',
  },
  form: {
    gap: 10,
  },
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
  button: {
    paddingVertical: 14,
    paddingHorizontal: 18,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  buttonPressed: {
    backgroundColor: '#1740a8',
  },
  buttonText: {
    fontSize: 17,
    fontWeight: '700',
    color: '#ffffff',
  },
  ghostButton: {
    paddingVertical: 14,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
    alignItems: 'center',
  },
  ghostButtonText: {
    fontSize: 17,
    fontWeight: '600',
    color: '#1d3a2f',
  },
  status: {
    fontSize: 15,
    fontWeight: '600',
    color: '#1d4ed8',
  },
  error: {
    fontSize: 15,
    fontWeight: '600',
    color: '#b42318',
  },
  center: {
    alignItems: 'center',
    gap: 8,
    paddingVertical: 24,
  },
  muted: {
    fontSize: 15,
    color: '#5b6573',
  },
  list: {
    gap: 12,
  },
  editRow: {
    gap: 10,
    padding: 12,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  editButtons: {
    flexDirection: 'row',
    gap: 10,
  },
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
  taskPressed: {
    backgroundColor: '#e4ebf5',
  },
  taskCheck: {
    fontSize: 22,
    width: 26,
    textAlign: 'center',
    color: '#10131a',
  },
  taskBody: {
    flex: 1,
  },
  taskTitle: {
    fontSize: 18,
    color: '#10131a',
  },
  taskTitleDone: {
    textDecorationLine: 'line-through',
    color: '#5b6573',
  },
  taskDue: {
    fontSize: 14,
    color: '#5b6573',
    marginTop: 2,
  },
});
