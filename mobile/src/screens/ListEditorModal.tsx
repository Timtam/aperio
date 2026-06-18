import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { Section } from '@aperio/shared';

import { useListFocusManager } from '../a11y/useListFocusManager';
import {
  createSection,
  deleteSection,
  deleteTaskList,
  getSections,
  reparentTaskList,
  updateSection,
} from '../api/client';
import { RadioGroup } from '../components/RadioGroup';
import type { RootStackScreenProps } from '../navigation/types';
import { useTaskStore } from '../state/taskStoreContext';

// Manage a single LOCAL task list (the sub-5 piece of the tasks port): reparent
// it (nest under another list / promote to top level), manage its sections
// (create / rename / delete), and delete the list. External lists are managed by
// their provider (writes are Unsupported on mobile), so the Tasks screen only
// surfaces "Manage" for local lists. Renaming a LIST is a container-override on
// the desktop (deferred on mobile with the rest of the overrides system), so it
// is intentionally absent here; SECTION rename is a plain field update and is
// included.
//
// Screen-reader-first: the parent picker is an accessible RadioGroup (selecting
// an option reparents immediately); each section is its own row with Rename +
// Delete; add/remove move SR focus via useListFocusManager; results announced.

const TOP_LEVEL = '__top__';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function ListEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'ListEditor'>) {
  const { listId } = route.params;
  const { t } = useTranslation();
  const { taskLists, refreshTaskLists, invalidateData } = useTaskStore();

  const list = taskLists.find((l) => l.id === listId);

  const [sections, setSections] = useState<Section[]>([]);
  const [newSectionName, setNewSectionName] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const loadSections = useCallback(async () => {
    try {
      setSections(await getSections(listId));
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [listId]);

  useEffect(() => {
    void loadSections();
  }, [loadSections]);

  // Eligible parents = every OTHER local list that isn't a descendant of this
  // one (a list can't be nested under itself or its own child — that would make
  // a cycle). Plus the "top level" sentinel.
  const parentOptions = useMemo(() => {
    const childrenOf = new Map<string | null, string[]>();
    for (const l of taskLists) {
      const arr = childrenOf.get(l.parent_id) ?? [];
      arr.push(l.id);
      childrenOf.set(l.parent_id, arr);
    }
    const banned = new Set<string>([listId]);
    const stack = [listId];
    while (stack.length > 0) {
      const cur = stack.pop() as string;
      for (const child of childrenOf.get(cur) ?? []) {
        if (!banned.has(child)) {
          banned.add(child);
          stack.push(child);
        }
      }
    }
    const opts = [{ value: TOP_LEVEL, label: t('sidebar.menu.moveToTopLevel') }];
    for (const l of taskLists) {
      if (l.account_id === 'local' && !banned.has(l.id)) {
        opts.push({ value: l.id, label: l.name });
      }
    }
    return opts;
  }, [listId, t, taskLists]);

  const reparent = useCallback(
    async (value: string) => {
      const parentId = value === TOP_LEVEL ? null : value;
      setError(null);
      setBusy(true);
      try {
        await reparentTaskList(listId, parentId);
        await refreshTaskLists();
        invalidateData();
        const name = list?.name ?? '';
        const parent = parentId
          ? (taskLists.find((l) => l.id === parentId)?.name ?? '')
          : null;
        announce(
          parent != null
            ? t('sidebar.menu.reparentedAnnouncement', { name, parent })
            : t('sidebar.menu.reparentedTopAnnouncement', { name }),
        );
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, invalidateData, list, listId, refreshTaskLists, t, taskLists],
  );

  const focus = useListFocusManager(sections.length);

  const addSection = useCallback(async () => {
    const name = newSectionName.trim();
    if (name.length === 0) return;
    setError(null);
    setBusy(true);
    try {
      // Append after the current last section.
      const position = sections.reduce((max, s) => Math.max(max, s.order + 1), 0);
      focus.onAdd();
      await createSection({ list_id: listId, name, position });
      setNewSectionName('');
      await loadSections();
      invalidateData();
      announce(t('dialogs.task.section.created', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, focus, invalidateData, listId, loadSections, newSectionName, sections, t]);

  const saveRename = useCallback(async () => {
    const section = sections.find((s) => s.id === editingId);
    if (section == null) return;
    const name = editingName.trim();
    if (name.length === 0) return;
    setError(null);
    setBusy(true);
    try {
      await updateSection({ ...section, name });
      setEditingId(null);
      await loadSections();
      invalidateData();
      announce(t('dialogs.task.section.renamed', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, editingId, editingName, invalidateData, loadSections, sections, t]);

  const removeSection = useCallback(
    async (section: Section, index: number) => {
      setError(null);
      setBusy(true);
      try {
        // Deleting a section is non-destructive to its tasks (they fall back to
        // ungrouped), so no confirmation — just a clear announcement.
        focus.onRemove(index);
        await deleteSection(section.id, listId);
        await loadSections();
        invalidateData();
        announce(t('dialogs.task.section.deleted', { name: section.name }));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, focus, invalidateData, listId, loadSections, t],
  );

  const removeList = useCallback(() => {
    if (list == null) return;
    Alert.alert(
      t('dialogs.confirm.deleteTaskListTitle'),
      t('dialogs.confirm.deleteTaskListMessage', { name: list.name }),
      [
        { text: t('dialogs.confirm.cancel'), style: 'cancel' },
        {
          text: t('mobile.delete'),
          style: 'destructive',
          onPress: () => {
            void (async () => {
              setError(null);
              setBusy(true);
              try {
                await deleteTaskList(listId);
                await refreshTaskLists();
                invalidateData();
                announce(t('mobile.deleted', { title: list.name }));
                navigation.goBack();
              } catch (err) {
                const message = errorMessage(err);
                setError(message);
                announce(t('mobile.error', { message }));
                setBusy(false);
              }
            })();
          },
        },
      ],
    );
  }, [announce, invalidateData, list, listId, navigation, refreshTaskLists, t]);

  if (list == null) {
    // The list was deleted (e.g. from another device's sync) while this modal
    // was open — nothing to manage.
    return (
      <View style={styles.screen}>
        <Text style={styles.muted} accessibilityRole="text">
          {t('mobile.noLists')}
        </Text>
      </View>
    );
  }

  const currentParent = list.parent_id ?? TOP_LEVEL;

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <Text style={styles.title} accessibilityRole="header">
        {list.name}
      </Text>

      {error != null && (
        <Text
          style={styles.error}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {error}
        </Text>
      )}

      {/* Reparent — only meaningful when there's at least one other list to nest
          under (besides the always-present "top level" option). */}
      {parentOptions.length > 1 && (
        <RadioGroup
          label={t('sidebar.menu.moveUnder')}
          value={currentParent}
          options={parentOptions}
          onChange={(v) => void reparent(v)}
          disabled={busy}
        />
      )}

      {/* Sections */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('mobile.sectionsHeading')}
      </Text>

      <View style={styles.addRow}>
        <TextInput
          style={styles.input}
          value={newSectionName}
          onChangeText={setNewSectionName}
          placeholder={t('dialogs.task.section.namePlaceholder')}
          accessibilityLabel={t('dialogs.task.section.newLabel')}
          returnKeyType="done"
          onSubmitEditing={() => void addSection()}
        />
        <Pressable
          ref={focus.registerAdd}
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={t('dialogs.task.section.addAction')}
          disabled={busy}
          onPress={() => void addSection()}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>
            {t('dialogs.task.section.create')}
          </Text>
        </Pressable>
      </View>

      {sections.length === 0 ? (
        <Text style={styles.muted} accessibilityRole="text">
          {t('dialogs.task.noSection')}
        </Text>
      ) : (
        sections.map((section, index) =>
          editingId === section.id ? (
            <View key={section.id} style={styles.sectionRow}>
              <TextInput
                style={styles.input}
                value={editingName}
                onChangeText={setEditingName}
                accessibilityLabel={t('dialogs.task.section.renameLabel')}
                autoFocus
                returnKeyType="done"
                onSubmitEditing={() => void saveRename()}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.task.section.save')}
                disabled={busy}
                onPress={() => void saveRename()}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>
                  {t('dialogs.task.section.save')}
                </Text>
              </Pressable>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.task.section.cancel')}
                onPress={() => setEditingId(null)}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>
                  {t('dialogs.task.section.cancel')}
                </Text>
              </Pressable>
            </View>
          ) : (
            <View key={section.id} style={styles.sectionRow}>
              <Text
                ref={focus.registerRow(index)}
                style={styles.sectionName}
                accessibilityRole="text"
              >
                {section.name}
              </Text>
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={`${t('dialogs.task.section.rename')}: ${section.name}`}
                disabled={busy}
                onPress={() => {
                  setEditingId(section.id);
                  setEditingName(section.name);
                }}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>
                  {t('dialogs.task.section.rename')}
                </Text>
              </Pressable>
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={`${t('dialogs.task.section.delete')}: ${section.name}`}
                disabled={busy}
                onPress={() => void removeSection(section, index)}
                style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
              >
                <Text style={styles.smallButtonText}>
                  {t('dialogs.task.section.delete')}
                </Text>
              </Pressable>
            </View>
          ),
        )
      )}

      {/* Delete the whole list (its tasks cascade away) — confirmed. */}
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: busy }}
        accessibilityLabel={t('dialogs.confirm.deleteTaskListTitle')}
        disabled={busy}
        onPress={removeList}
        style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
      >
        <Text style={styles.deleteButtonText}>
          {t('dialogs.confirm.deleteTaskListTitle')}
        </Text>
      </Pressable>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 16 },
  title: { fontSize: 22, fontWeight: '700', color: '#10131a' },
  heading: { fontSize: 17, fontWeight: '700', color: '#2b3240' },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  muted: { fontSize: 15, color: '#5b6573' },
  addRow: { flexDirection: 'row', gap: 10, alignItems: 'center' },
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
  addButton: {
    paddingVertical: 12,
    paddingHorizontal: 16,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
  },
  addButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  sectionRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 8,
  },
  sectionName: { flex: 1, fontSize: 17, color: '#10131a' },
  smallButton: {
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  smallButtonText: { fontSize: 15, fontWeight: '600', color: '#1d4ed8' },
  pressed: { opacity: 0.7 },
  deleteButton: {
    marginTop: 8,
    paddingVertical: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#f0c2bd',
    backgroundColor: '#fdecea',
    alignItems: 'center',
  },
  deleteButtonText: { fontSize: 16, fontWeight: '700', color: '#b42318' },
});
