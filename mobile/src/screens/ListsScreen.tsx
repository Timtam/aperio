import { useCallback, useEffect, useRef, useState } from 'react';
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

import type { TaskList } from '@aperio/shared';

import { createTaskList } from '../api/client';
import { useTaskStore } from '../state/taskStoreContext';

// Task-list management — a minimal-but-real stub (sub-5 fleshes it out:
// rename / delete / reparent, sections management, colour labels, sharing).
// It proves the catalog read, the selection Set + reconciler, and list
// creation feeding back into the store.

export default function ListsScreen() {
  const { t } = useTranslation();
  const { taskLists, selectedTaskListIds, toggleTaskList, refreshTaskLists } =
    useTaskStore();

  const [newName, setNewName] = useState('');
  const [error, setError] = useState<string | null>(null);

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // After a create, move screen-reader focus to the new row once the refreshed
  // catalog re-renders.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [taskLists]);

  const addList = useCallback(async () => {
    const name = newName.trim();
    if (name.length === 0) return;
    setError(null);
    try {
      const created = await createTaskList({ name, embedded_in_calendar: null });
      setNewName('');
      await refreshTaskLists();
      pendingFocusId.current = created.id;
      announce(t('mobile.listAdded', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    }
  }, [announce, newName, refreshTaskLists, t]);

  const onToggle = useCallback(
    (list: TaskList) => {
      const wasSelected = selectedTaskListIds.has(list.id);
      toggleTaskList(list.id);
      announce(
        wasSelected
          ? t('mobile.listDeselected', { name: list.name })
          : t('mobile.listSelected', { name: list.name }),
      );
    },
    [announce, selectedTaskListIds, t, toggleTaskList],
  );

  return (
    <View style={styles.screen}>
      <View style={styles.form}>
        <TextInput
          style={styles.input}
          value={newName}
          onChangeText={setNewName}
          placeholder={t('mobile.newListPlaceholder')}
          accessibilityLabel={t('mobile.newListLabel')}
          returnKeyType="done"
          onSubmitEditing={() => void addList()}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.add')}
          onPress={() => void addList()}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('mobile.add')}</Text>
        </Pressable>
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text">
          {error}
        </Text>
      )}

      {taskLists.length === 0 ? (
        <Text style={styles.muted}>{t('mobile.noLists')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {taskLists.map((list) => {
            const selected = selectedTaskListIds.has(list.id);
            return (
              <Pressable
                key={list.id}
                ref={(node) => {
                  rowTags.current[list.id] = node ? findNodeHandle(node) : null;
                }}
                accessible
                accessibilityRole="checkbox"
                accessibilityState={{ checked: selected }}
                accessibilityLabel={list.name}
                onPress={() => onToggle(list)}
                style={({ pressed }) => [styles.row, pressed && styles.rowPressed]}
              >
                <Text style={styles.check}>{selected ? '☑' : '☐'}</Text>
                <Text style={styles.listName}>{list.name}</Text>
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
  form: { flexDirection: 'row', gap: 10, padding: 16, alignItems: 'center' },
  input: {
    flex: 1,
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
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  buttonPressed: { backgroundColor: '#1740a8' },
  buttonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  list: { gap: 12, padding: 16 },
  row: {
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
  check: { fontSize: 22, width: 26, textAlign: 'center', color: '#10131a' },
  listName: { flex: 1, fontSize: 18, color: '#10131a' },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318', paddingHorizontal: 16 },
});
