import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { useListFocusManager } from '../a11y/useListFocusManager';
import {
  createSection,
  deleteSection,
  deleteTaskList,
  reparentTaskList,
  updateSection,
} from '../api/client';
import {
  renameContainer,
  setContainerColorLabel,
  setSectionColor,
} from '../api/containerColor';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { SoundSelect } from '../components/SoundSelect';
import type { RootStackScreenProps } from '../navigation/types';
import {
  readTaskBehaviour,
  withListOverride,
  writeListOverrides,
  type CarryOverDefault,
  type ListOverrides,
} from '../state/taskBehaviour';
import { useSoundPref } from '../state/useSoundPref';
import { useTaskStore } from '../state/taskStoreContext';
import { useThemedStyles, type ThemeColors } from '../theme';

/** Tri-state mapping for a per-list boolean override: undefined = inherit the
 *  global, true = on, false = off. */
type TriState = 'inherit' | 'on' | 'off';
const toTri = (v: boolean | undefined): TriState =>
  v === undefined ? 'inherit' : v ? 'on' : 'off';
const fromTri = (s: TriState): boolean | undefined =>
  s === 'inherit' ? undefined : s === 'on';

// Manage a single task list (the sub-5 piece of the tasks port): reparent it
// (nest under another list / promote to top level), manage its sections (create
// / rename / delete), and delete the list.
//
// Capability gating (matches the desktop, which gates affordances on
// task_capabilities):
//   - Reparent + delete a LIST route only to the LOCAL store on mobile
//     (reparent_task_list rejects external lists; create/delete list are
//     local-only), so those two are shown only for the local account.
//   - SECTIONS create/rename/delete ROUTE to the owning provider, so they're
//     offered for local lists (the local store supports sections) and for
//     external lists whose adapter reports `manageable_sections`.
// Renaming a LIST is a container-override on the desktop (deferred on mobile
// with the rest of the overrides system, like colour labels + sharing), so it's
// intentionally absent.
//
// Screen-reader-first: the parent picker is an accessible RadioGroup (selecting
// an option reparents immediately); each section is its own row with Rename +
// Delete; add/remove/rename move SR focus via useListFocusManager; section
// mutations refresh the SHARED store cache so the grouped Tasks screen regroups;
// results announced.

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
  const styles = useThemedStyles(makeStyles);
  const {
    taskLists,
    refreshTaskLists,
    invalidateData,
    loadSections,
    sectionsByList,
    colorLabels,
  } = useTaskStore();

  const list = taskLists.find((l) => l.id === listId);
  // Sections live in the SHARED store cache (the same `sectionsByList` the Tasks
  // screen groups by), so mutating them here regroups there. Never a private
  // copy — that was the bug a private copy reintroduces. Memoised so its
  // identity is stable across renders (the callbacks below depend on it).
  const sections = useMemo(
    () => sectionsByList[listId] ?? [],
    [sectionsByList, listId],
  );

  const [newSectionName, setNewSectionName] = useState('');
  // The list's own name, edited in place (local lists only).
  const [renameText, setRenameText] = useState(() => list?.name ?? '');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState('');
  // The section being edited also carries its colour ('' = none); only LOCAL
  // sections store it on their row (external = override, deferred).
  const [editingColor, setEditingColor] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // This list's default reminder sound (§14.4 container level), host-local +
  // inheritable. Offered for every list since the sound pref applies to any
  // container's reminders.
  const sound = useSoundPref(`sound.tasklist.${listId}`);
  // This list's task-behaviour override (§ Settings → Tasks per-list): the full
  // synced map, of which this list's entry (or {} = inherit globals) is edited
  // here. Read once on open; each change persists the whole map.
  const [overrides, setOverrides] = useState<Record<string, ListOverrides>>({});

  // Index of the row currently being renamed, so SR focus can be restored to it
  // on Save/Cancel (a rename doesn't change the row count, so the focus
  // manager's count-keyed effect won't fire on its own).
  const renameIndex = useRef<number | null>(null);
  const pendingRenameFocus = useRef<number | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // Load (refresh) this list's sections into the shared store on open.
  useEffect(() => {
    void loadSections(listId).catch((err) => setError(errorMessage(err)));
  }, [listId, loadSections]);

  // Load the per-list task-behaviour override map on open.
  useEffect(() => {
    void readTaskBehaviour().then((b) => setOverrides(b.listOverrides));
  }, []);

  // This list's current override entry (or {} = inherit globals).
  const listOverride: ListOverrides = overrides[listId] ?? {};

  // Patch one field of this list's override, persisting the whole synced map.
  const patchOverride = useCallback(
    (patch: ListOverrides) => {
      setOverrides((prev) => {
        const merged = { ...(prev[listId] ?? {}), ...patch };
        const next = withListOverride(prev, listId, merged);
        void writeListOverrides(next);
        return next;
      });
    },
    [listId],
  );

  const isLocal = list?.account_id === 'local';
  const canReparent = isLocal;
  const canDeleteList = isLocal;
  const canManageSections =
    isLocal || (list?.task_capabilities?.manageable_sections ?? false);
  // Membership/sharing (§9.7) — only for external lists whose adapter declares
  // it; opens the dedicated members manager. Local lists have no sharing concept.
  const canManageMembers = list?.task_capabilities?.manageable ?? false;

  // Eligible parents = every OTHER local list that isn't a descendant of this
  // one (no cycles). Plus the "top level" sentinel.
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
      if (busy) return;
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
    [announce, busy, invalidateData, list, listId, refreshTaskLists, t, taskLists],
  );

  // Bind (or clear) this LOCAL list's colour label — rides its own synced row.
  // The picker fires per selection, like the parent picker above.
  const setColour = useCallback(
    async (colorLabelId: string) => {
      if (busy) return;
      setError(null);
      setBusy(true);
      try {
        await setContainerColorLabel(listId, 'task_list', colorLabelId || null);
        await refreshTaskLists();
        invalidateData();
        const name = list?.name ?? '';
        const colour = colorLabelId
          ? colorLabels.find((l) => l.id === colorLabelId)?.name
          : undefined;
        announce(
          colour != null
            ? t('sidebar.menu.colorSetAnnouncement', { name, color: colour })
            : t('sidebar.menu.colorClearedAnnouncement', { name }),
        );
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, colorLabels, invalidateData, list, listId, refreshTaskLists, t],
  );

  // Rename this LOCAL list — the new name rides its own synced row.
  const renameList = useCallback(async () => {
    if (busy) return;
    const name = renameText.trim();
    if (name.length === 0 || name === list?.name) return;
    setError(null);
    setBusy(true);
    try {
      await renameContainer(listId, 'task_list', name);
      await refreshTaskLists();
      invalidateData();
      announce(t('mobile.listRenamed', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, busy, invalidateData, list, listId, refreshTaskLists, renameText, t]);

  const focus = useListFocusManager(sections.length);

  // Restore SR focus to the renamed row once it reverts from the TextInput back
  // to its name Text (editingId cleared) — the count is unchanged so the focus
  // manager's own effect won't fire.
  useEffect(() => {
    if (editingId !== null) return;
    const i = pendingRenameFocus.current;
    if (i == null) return;
    pendingRenameFocus.current = null;
    focus.focusRow(i);
  }, [editingId, focus]);

  const addSection = useCallback(async () => {
    if (busy) return;
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
      await loadSections(listId);
      invalidateData();
      announce(t('dialogs.task.section.created', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [
    announce,
    busy,
    focus,
    invalidateData,
    listId,
    loadSections,
    newSectionName,
    sections,
    t,
  ]);

  const saveRename = useCallback(async () => {
    if (busy) return;
    const section = sections.find((s) => s.id === editingId);
    if (section == null) return;
    const name = editingName.trim();
    if (name.length === 0) return;
    setError(null);
    setBusy(true);
    try {
      // Name via update_section (renames the row / pushes to the provider);
      // colour via set_section_color, which routes local → row, external →
      // host-local override (update_section ignores colour for external).
      await updateSection({ ...section, name });
      await setSectionColor(section.id, listId, editingColor || null);
      pendingRenameFocus.current = renameIndex.current;
      setEditingId(null);
      await loadSections(listId);
      invalidateData();
      announce(t('dialogs.task.section.renamed', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [
    announce,
    busy,
    editingColor,
    editingId,
    editingName,
    invalidateData,
    listId,
    loadSections,
    sections,
    t,
  ]);

  const cancelRename = useCallback(() => {
    pendingRenameFocus.current = renameIndex.current;
    setEditingId(null);
  }, []);

  const removeSection = useCallback(
    async (sectionId: string, sectionName: string, index: number) => {
      if (busy) return;
      setError(null);
      setBusy(true);
      try {
        // Deleting a section is non-destructive to its tasks (they fall back to
        // ungrouped), so no confirmation — just a clear announcement.
        focus.onRemove(index);
        await deleteSection(sectionId, listId);
        await loadSections(listId);
        invalidateData();
        announce(t('dialogs.task.section.deleted', { name: sectionName }));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, focus, invalidateData, listId, loadSections, t],
  );

  const removeList = useCallback(() => {
    if (list == null || busy) return;
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
  }, [announce, busy, invalidateData, list, listId, navigation, refreshTaskLists, t]);

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
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
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

      {/* Rename — every list. A local list renames its own synced row; an
          external list pushes the rename to its provider (falling back to a
          host-local name override). */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('mobile.renameListLabel')}
      </Text>
      <View style={styles.addRow}>
        <TextInput
          style={styles.input}
          value={renameText}
          onChangeText={setRenameText}
          accessibilityLabel={t('mobile.renameListLabel')}
          editable={!busy}
          returnKeyType="done"
          onSubmitEditing={() => void renameList()}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={t('mobile.rename')}
          disabled={busy}
          onPress={() => void renameList()}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>{t('mobile.rename')}</Text>
        </Pressable>
      </View>

      {/* Reparent — local lists only (the backend rejects external reparent),
          and only when there's at least one other list to nest under. */}
      {canReparent && parentOptions.length > 1 && (
        <RadioGroup
          label={t('sidebar.menu.moveUnder')}
          value={currentParent}
          options={parentOptions}
          onChange={(v) => void reparent(v)}
          disabled={busy}
        />
      )}

      {/* Colour — every list. A local list binds it on its own synced row; an
          external list stores a host-local colour override (the read path stamps
          it back). Real swatches for sighted users + the name for SR. */}
      <ColorLabelSelect
        value={list.color_label ?? ''}
        labels={colorLabels}
        onChange={(id) => void setColour(id)}
        disabled={busy}
      />

      {/* Default reminder sound (§14.4 container level) — System / Silent / use
          the global default. Host-local + inheritable. */}
      {!sound.loading && (
        <SoundSelect
          label={t('reminders.sound.label')}
          value={sound.value}
          allowInherit
          onChange={(next) => void sound.save(next)}
          disabled={busy}
        />
      )}

      {/* Per-list task-behaviour overrides (§ Settings → Tasks "Per task list"):
          override the global status-coupling / auto-date / carry-over for THIS
          list. "Global default" inherits. Applies to the check-off path + the
          day-start checkers on every device (synced). */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.tasks.perList.heading')}
      </Text>
      <RadioGroup<TriState>
        label={t('dialogs.tasks.perList.cascade')}
        value={toTri(listOverride.cascade)}
        options={[
          { value: 'inherit', label: t('dialogs.tasks.perList.inherit') },
          { value: 'on', label: t('dialogs.tasks.perList.on') },
          { value: 'off', label: t('dialogs.tasks.perList.off') },
        ]}
        onChange={(v) => patchOverride({ cascade: fromTri(v) })}
        disabled={busy}
      />
      <RadioGroup<TriState>
        label={t('dialogs.tasks.perList.autoDate')}
        value={toTri(listOverride.autoDate)}
        options={[
          { value: 'inherit', label: t('dialogs.tasks.perList.inherit') },
          { value: 'on', label: t('dialogs.tasks.perList.on') },
          { value: 'off', label: t('dialogs.tasks.perList.off') },
        ]}
        onChange={(v) => patchOverride({ autoDate: fromTri(v) })}
        disabled={busy}
      />
      <RadioGroup<string>
        label={t('dialogs.tasks.perList.carryOver')}
        value={listOverride.carryOverDefault ?? 'inherit'}
        options={[
          { value: 'inherit', label: t('dialogs.tasks.perList.inherit') },
          { value: 'ask', label: t('dialogs.tasks.carryOverDefault.options.ask') },
          { value: 'today', label: t('dialogs.tasks.carryOverDefault.options.today') },
          { value: 'backlog', label: t('dialogs.tasks.carryOverDefault.options.backlog') },
        ]}
        onChange={(v) =>
          patchOverride({
            carryOverDefault: v === 'inherit' ? undefined : (v as CarryOverDefault),
          })
        }
        disabled={busy}
      />

      {/* Members / sharing — external lists whose adapter declares it (§9.7);
          opens the dedicated members manager. */}
      {canManageMembers && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('mobile.manageMembersLabel')}: ${list.name}`}
          onPress={() =>
            navigation.navigate('TaskMembers', { listId, listName: list.name })
          }
          style={({ pressed }) => [styles.manageMembersButton, pressed && styles.pressed]}
        >
          <Text style={styles.manageMembersText}>{t('mobile.manageMembersLabel')}</Text>
        </Pressable>
      )}

      {/* Sections — local lists, or external lists whose provider can manage
          sections at the source. */}
      {canManageSections && (
        <>
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
              editable={!busy}
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
            sections.map((section, index) => {
              // The section's bound colour (LOCAL only) — a swatch for sighted
              // users + the name on the row's accessible label.
              const colour = section.color_label
                ? colorLabels.find((l) => l.id === section.color_label)
                : undefined;
              return editingId === section.id ? (
                <View key={section.id} style={styles.sectionEditPanel}>
                  <TextInput
                    style={styles.input}
                    value={editingName}
                    onChangeText={setEditingName}
                    accessibilityLabel={t('dialogs.task.section.renameLabel')}
                    editable={!busy}
                    autoFocus
                    returnKeyType="done"
                    onSubmitEditing={() => void saveRename()}
                  />
                  {/* Colour — every section: a local section binds it on its
                      row, an external one via a host-local override (set on
                      Save through set_section_color). */}
                  <ColorLabelSelect
                    value={editingColor}
                    labels={colorLabels}
                    onChange={setEditingColor}
                    disabled={busy}
                  />
                  <View style={styles.editButtons}>
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
                      onPress={cancelRename}
                      style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.smallButtonText}>
                        {t('dialogs.task.section.cancel')}
                      </Text>
                    </Pressable>
                  </View>
                </View>
              ) : (
                <View key={section.id} style={styles.sectionRow}>
                  {colour != null && (
                    <View
                      accessible={false}
                      importantForAccessibility="no"
                      style={[styles.colorDot, { backgroundColor: colour.hex }]}
                    />
                  )}
                  <Text
                    ref={focus.registerRow(index)}
                    style={styles.sectionName}
                    accessibilityRole="text"
                    accessibilityLabel={
                      colour != null
                        ? `${section.name}${t('mobile.colorLabelSuffix', { name: colour.name })}`
                        : undefined
                    }
                  >
                    {section.name}
                  </Text>
                  {/* Edit covers name + colour for a local section; an external
                      section (manageable_sections) can only be renamed. */}
                  <Pressable
                    accessibilityRole="button"
                    accessibilityState={{ disabled: busy }}
                    accessibilityLabel={`${
                      isLocal ? t('mobile.edit') : t('dialogs.task.section.rename')
                    }: ${section.name}`}
                    disabled={busy}
                    onPress={() => {
                      renameIndex.current = index;
                      setEditingId(section.id);
                      setEditingName(section.name);
                      setEditingColor(section.color_label ?? '');
                    }}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>
                      {isLocal ? t('mobile.edit') : t('dialogs.task.section.rename')}
                    </Text>
                  </Pressable>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityState={{ disabled: busy }}
                    accessibilityLabel={`${t('dialogs.task.section.delete')}: ${section.name}`}
                    disabled={busy}
                    onPress={() =>
                      void removeSection(section.id, section.name, index)
                    }
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>
                      {t('dialogs.task.section.delete')}
                    </Text>
                  </Pressable>
                </View>
              );
            })
          )}
        </>
      )}

      {/* Delete the whole list (local only; its tasks cascade away) — confirmed. */}
      {canDeleteList && (
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
      )}
    </FormScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    title: { fontSize: 22, fontWeight: '700', color: c.textPrimary },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    muted: { fontSize: 15, color: c.textSecondary },
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
      backgroundColor: c.surface,
    },
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      backgroundColor: c.accent,
    },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    sectionRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 8,
    },
    // Edit mode stacks the name field, the colour picker and the buttons.
    sectionEditPanel: { gap: 10, paddingVertical: 8 },
    editButtons: { flexDirection: 'row', gap: 10 },
    // The section's bound colour (sighted users); subtle border keeps light
    // colours visible. Matches the task/event/list row dot.
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    sectionName: { flex: 1, fontSize: 17, color: c.textPrimary },
    smallButton: {
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    smallButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    manageMembersButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    manageMembersText: { fontSize: 16, fontWeight: '700', color: c.accent },
    pressed: { opacity: 0.7 },
    deleteButton: {
      marginTop: 8,
      paddingVertical: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
      alignItems: 'center',
    },
    deleteButtonText: { fontSize: 16, fontWeight: '700', color: c.danger },
  });
