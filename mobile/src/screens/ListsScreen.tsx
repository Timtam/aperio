import { useNavigation } from '@react-navigation/native';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { TaskList } from '@aperio/shared';

import { listAccounts } from '../api/accounts';
import { createTaskList } from '../api/client';
import { useCacheReload } from '../state/cacheObserver';
import { useTaskStore } from '../state/taskStoreContext';
import { useRefreshErrors } from '../state/useRefreshErrors';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Task-list catalog: read the lists, toggle which are shown (the selection Set +
// reconciler), and create a top-level local list. A "Manage" entry opens the
// ListEditor modal: local lists can be reparented / have their sections managed
// / recoloured / be deleted; external lists are provider-managed, so on mobile
// only their SECTION management routes through (reparent + delete + recolour a
// LIST are local-only) — hence external lists get "Manage" only when their
// adapter reports manageable_sections. Each row shows the list's bound colour
// as a real swatch for sighted users (name rides the accessible label). List
// rename is a container name-override (deferred with the overrides system).

export default function ListsScreen() {
  const { t } = useTranslation();
  // Per-account refresh-error surface (silent-staleness warning banner).
  const { errorsByAccount } = useRefreshErrors();
  const navigation = useNavigation();
  const { taskLists, selectedTaskListIds, toggleTaskList, refreshTaskLists, colorLabels } =
    useTaskStore();
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();

  const [newName, setNewName] = useState('');
  const [error, setError] = useState<string | null>(null);

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  // Resolve each list's owning account so a row can show which account it
  // belongs to (local lists read as "On this device").
  const [accountNames, setAccountNames] = useState<Map<string, string>>(new Map());
  useEffect(() => {
    let cancelled = false;
    void listAccounts()
      .then((list) => {
        if (!cancelled) {
          setAccountNames(new Map(list.map((a) => [a.id, a.display_name])));
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

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

  // Live-update the list catalog when an external task-cache refresh lands (the
  // root observer already announced it politely). Reuses the store's own
  // catalog reload; fire-and-forget so the hook's reload stays `() => void`.
  const reloadLists = useCallback(() => {
    void refreshTaskLists().catch(() => {});
  }, [refreshTaskLists]);
  useCacheReload('tasks', reloadLists);

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

  // A native `switch`-role control (below): VoiceOver/TalkBack announce the new
  // on/off state as part of the toggle itself — reliably, unlike the
  // announceForAccessibility that used to sit here and was being dropped (the
  // user heard no feedback). So no manual announce is needed.
  const onToggle = useCallback(
    (list: TaskList) => toggleTaskList(list.id),
    [toggleTaskList],
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
          accessibilityLabel={t('mobile.newListLabel')}
          onPress={() => void addList()}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('mobile.newListLabel')}</Text>
        </Pressable>
      </View>

      {errorsByAccount.size > 0 && (
        <Text style={styles.refreshWarning} accessibilityRole="text">
          {t(
            [...errorsByAccount.values()].some((a) => a.auth_suspected)
              ? 'refreshErrors.bannerAuth'
              : 'refreshErrors.banner',
          )}
        </Text>
      )}
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
            // Every list is manageable now (the editor gates each control): a
            // local list reparents / deletes / binds colour+name on its own row;
            // an external list can be recoloured + renamed (host-local override /
            // provider rename) and its sections managed when the adapter allows.
            // The list's bound colour: a swatch for sighted users + the name on
            // the accessible label so colour isn't the only signal.
            const colour = list.color_label
              ? colorLabels.find((l) => l.id === list.color_label)
              : undefined;
            const label = colour
              ? `${list.name}${t('mobile.colorLabelSuffix', { name: colour.name })}`
              : list.name;
            const accountName =
              list.account_id === 'local'
                ? t('sidebar.localAccount')
                : (accountNames.get(list.account_id) ?? list.account_id);
            const fullLabel = `${label}${t('mobile.accountSuffix', { name: accountName })}`;
            return (
              <View key={list.id} style={styles.row}>
                <Pressable
                  ref={(node) => {
                    rowTags.current[list.id] = node ? findNodeHandle(node) : null;
                  }}
                  accessible
                  accessibilityRole="switch"
                  accessibilityState={{ checked: selected }}
                  accessibilityLabel={fullLabel}
                  onPress={() => onToggle(list)}
                  style={({ pressed }) => [styles.rowToggle, pressed && styles.rowPressed]}
                >
                  {colour != null && (
                    <View
                      accessible={false}
                      importantForAccessibility="no"
                      style={[styles.colorDot, { backgroundColor: colour.hex }]}
                    />
                  )}
                  <View style={styles.rowText} importantForAccessibility="no">
                    <Text style={styles.listName} importantForAccessibility="no">
                      {list.name}
                    </Text>
                    <Text style={styles.listAccount} importantForAccessibility="no">
                      {accountName}
                    </Text>
                  </View>
                  {/* Visual only — the Pressable owns the toggle + the switch
                      a11y trait; hidden from the reader (matches SwitchRow). */}
                  <View pointerEvents="none">
                    <Switch
                      value={selected}
                      trackColor={{ false: colors.border, true: colors.accent }}
                      importantForAccessibility="no"
                      accessibilityElementsHidden
                    />
                  </View>
                </Pressable>
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`${t('mobile.manageList')}: ${list.name}`}
                  onPress={() => navigation.navigate('ListEditor', { listId: list.id })}
                  style={({ pressed }) => [styles.manageButton, pressed && styles.rowPressed]}
                >
                  <Text style={styles.manageButtonText}>{t('mobile.manageList')}</Text>
                </Pressable>
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

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    form: { flexDirection: 'row', gap: 10, padding: 16, alignItems: 'center' },
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
    button: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    list: { gap: 12, padding: 16 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      paddingVertical: 6,
      paddingHorizontal: 10,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowToggle: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 14,
      paddingVertical: 10,
      paddingHorizontal: 6,
    },
    rowPressed: { backgroundColor: c.surfacePressed },
    manageButton: {
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    manageButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    // The list's bound colour (sighted users); subtle border keeps light colours
    // visible on the card. Matches the task/event row dot.
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    rowText: { flex: 1 },
    listName: { fontSize: 18, color: c.textPrimary },
    listAccount: { fontSize: 13, color: c.textSecondary, marginTop: 2 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
    refreshWarning: {
      fontSize: 14,
      fontWeight: '600',
      color: c.danger,
      backgroundColor: c.dangerBg,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      borderRadius: 8,
      marginHorizontal: 16,
      marginBottom: 6,
      paddingHorizontal: 10,
      paddingVertical: 8,
    },
  });
