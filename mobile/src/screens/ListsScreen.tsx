import { useNavigation } from '@react-navigation/native';
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

// Task-list catalog: read the lists, toggle which are shown (the selection Set +
// reconciler), and create a top-level local list. Each LOCAL list opens a
// ListEditor modal (reparent / sections management / delete) via "Manage";
// external lists are provider-managed (writes Unsupported on mobile) so they
// only toggle. List rename is a container-override (deferred with the rest of
// the overrides system, like colour labels + sharing).

export default function ListsScreen() {
  const { t } = useTranslation();
  const navigation = useNavigation();
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
            // External task lists are managed by their provider (writes are
            // Unsupported on mobile), so only local lists get a "Manage" entry.
            const isLocal = list.account_id === 'local';
            return (
              <View key={list.id} style={styles.row}>
                <Pressable
                  ref={(node) => {
                    rowTags.current[list.id] = node ? findNodeHandle(node) : null;
                  }}
                  accessible
                  accessibilityRole="checkbox"
                  accessibilityState={{ checked: selected }}
                  accessibilityLabel={list.name}
                  onPress={() => onToggle(list)}
                  style={({ pressed }) => [styles.rowToggle, pressed && styles.rowPressed]}
                >
                  <Text style={styles.check}>{selected ? '☑' : '☐'}</Text>
                  <Text style={styles.listName}>{list.name}</Text>
                </Pressable>
                {isLocal && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={`${t('mobile.manageList')}: ${list.name}`}
                    onPress={() =>
                      navigation.navigate('ListEditor', { listId: list.id })
                    }
                    style={({ pressed }) => [
                      styles.manageButton,
                      pressed && styles.rowPressed,
                    ]}
                  >
                    <Text style={styles.manageButtonText}>
                      {t('mobile.manageList')}
                    </Text>
                  </Pressable>
                )}
              </View>
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
    gap: 10,
    paddingVertical: 6,
    paddingHorizontal: 10,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowToggle: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 14,
    paddingVertical: 10,
    paddingHorizontal: 6,
  },
  rowPressed: { backgroundColor: '#e4ebf5' },
  manageButton: {
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#ffffff',
  },
  manageButtonText: { fontSize: 15, fontWeight: '600', color: '#1d4ed8' },
  check: { fontSize: 22, width: 26, textAlign: 'center', color: '#10131a' },
  listName: { flex: 1, fontSize: 18, color: '#10131a' },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318', paddingHorizontal: 16 },
});
