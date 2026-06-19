import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import type { MemberRight, TaskListShare, TaskUser } from '@aperio/shared';

import {
  taskAddMember,
  taskListShares,
  taskRemoveMember,
  taskSearchUsers,
  taskSetMemberRight,
} from '../api/client';
import { RadioGroup } from '../components/RadioGroup';
import type { RootStackScreenProps } from '../navigation/types';
import { useTaskStore } from '../state/taskStoreContext';
import { useThemedStyles, type ThemeColors } from '../theme';

// Manage one task list's membership / sharing (DESIGN §9.7) — the mobile twin of
// the desktop TaskMembersDialog. Reachable only for lists whose adapter declares
// `manageable` (gated by the caller in the list editor). List the current shares
// with their right + pending state, add/invite people, remove them, change
// roles.
//
// The add control follows the adapter's `member_add_by`:
//   - `search` (Vikunja): debounced directory search → tap a hit to add.
//   - `email` (Todoist): type an email + Invite (pending until accepted; no
//     directory, no roles).
//
// Screen-reader-first: a labelled add field, a never-silent search status live
// region, each share its own row with an accessible permission RadioGroup +
// Remove, and every mutation announces its result.

const RIGHTS: MemberRight[] = ['read', 'write', 'admin'];

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function TaskMembersScreen({
  route,
  navigation,
}: RootStackScreenProps<'TaskMembers'>) {
  const { listId, listName } = route.params;
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { taskLists } = useTaskStore();

  // The list's name + capabilities come from the store (kept current); the param
  // name seeds the header before the store resolves.
  const list = taskLists.find((l) => l.id === listId);
  const name = list?.name ?? listName;
  const addByEmail = list?.task_capabilities?.member_add_by === 'email';

  const [shares, setShares] = useState<TaskListShare[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<TaskUser[]>([]);
  // Search lifecycle so the results area is never silent: idle (query too short)
  // → loading → done | error.
  const [searchState, setSearchState] = useState<'idle' | 'loading' | 'done' | 'error'>(
    'idle',
  );
  const [searchError, setSearchError] = useState('');
  const [busy, setBusy] = useState(false);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  useEffect(() => {
    navigation.setOptions({ title: t('dialogs.taskMembers.title', { list: name }) });
  }, [navigation, name, t]);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setShares(await taskListShares(listId));
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [listId]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // Debounced directory search (≥ 2 chars). Skipped entirely when the adapter
  // invites by email — there's no directory to query.
  useEffect(() => {
    if (addByEmail) return;
    const q = query.trim();
    if (q.length < 2) {
      setResults([]);
      setSearchState('idle');
      return;
    }
    let cancelled = false;
    setSearchState('loading');
    const handle = setTimeout(() => {
      void taskSearchUsers(listId, q)
        .then((r) => {
          if (cancelled) return;
          setResults(r);
          setSearchState('done');
        })
        .catch((err) => {
          if (cancelled) return;
          setResults([]);
          setSearchError(errorMessage(err));
          setSearchState('error');
        });
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [listId, query, addByEmail]);

  const run = useCallback(
    async (fn: () => Promise<void>) => {
      setBusy(true);
      try {
        await fn();
        await reload();
      } catch (err) {
        setError(errorMessage(err));
      } finally {
        setBusy(false);
      }
    },
    [reload],
  );

  const add = (user: TaskUser) =>
    void run(async () => {
      // Default new members to write; the permission picker can adjust it.
      await taskAddMember(listId, user.id, 'write');
      announce(t('dialogs.taskMembers.added', { name: user.name }));
      setQuery('');
      setResults([]);
    });

  // Email-invite path (Todoist): the typed text IS the member ref; there are no
  // roles, so `right` is null.
  const invite = () =>
    void run(async () => {
      const email = query.trim();
      if (!email) return;
      await taskAddMember(listId, email, null);
      announce(t('dialogs.taskMembers.added', { name: email }));
      setQuery('');
    });

  const remove = (share: TaskListShare) =>
    void run(async () => {
      await taskRemoveMember(listId, share.user.id);
      announce(t('dialogs.taskMembers.removed', { name: share.user.name }));
    });

  const changeRight = (share: TaskListShare, right: MemberRight) => {
    if (busy || share.right === right) return;
    const prev = share.right;
    // Optimistic in place: update just this row's right so the picker keeps its
    // value + focus. Revert only on failure (no full reload, which would rebuild
    // the rows and snap focus away — the desktop hit exactly this NVDA bug).
    setShares((rows) =>
      rows.map((r) => (r.user.id === share.user.id ? { ...r, right } : r)),
    );
    void taskSetMemberRight(listId, share.user.id, right)
      .then(() =>
        announce(t('dialogs.taskMembers.rightChanged', { name: share.user.name })),
      )
      .catch((err) => {
        setShares((rows) =>
          rows.map((r) => (r.user.id === share.user.id ? { ...r, right: prev } : r)),
        );
        setError(errorMessage(err));
      });
  };

  const existingIds = useMemo(() => new Set(shares.map((s) => s.user.id)), [shares]);
  // Directory matches not already on the list — the actually-addable set.
  const addable = results.filter((u) => !existingIds.has(u.id));

  // Single status line for the search area (search-mode adapters only). Never
  // silent: loading, failure, no-match (with the Vikunja exact-username caveat),
  // and the all-already-members case.
  let searchStatus = '';
  let searchStatusError = false;
  if (!addByEmail) {
    if (searchState === 'loading') {
      searchStatus = t('dialogs.taskMembers.searching');
    } else if (searchState === 'error') {
      searchStatus = t('dialogs.taskMembers.searchError', { error: searchError });
      searchStatusError = true;
    } else if (searchState === 'done') {
      if (results.length === 0) {
        searchStatus = t('dialogs.taskMembers.noResults', { query: query.trim() });
      } else if (addable.length === 0) {
        searchStatus = t('dialogs.taskMembers.allAlreadyMembers');
      } else {
        searchStatus = t('dialogs.taskMembers.searchResults', { count: addable.length });
      }
    }
  }

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLiveRegion="polite">
          {t('dialogs.taskMembers.loading')}
        </Text>
      ) : shares.length === 0 ? (
        <Text style={styles.muted} accessibilityRole="text">
          {t('dialogs.taskMembers.empty')}
        </Text>
      ) : (
        <View accessibilityRole="list" style={styles.list}>
          {shares.map((s) => (
            <View key={s.user.id} style={styles.shareRow}>
              <Text style={styles.shareName} accessibilityRole="text">
                {s.user.name}
                {s.pending ? ` · ${t('dialogs.taskMembers.pending')}` : ''}
              </Text>
              {s.right !== null && (
                <RadioGroup<MemberRight>
                  label={t('dialogs.taskMembers.rightFor', { name: s.user.name })}
                  value={s.right}
                  options={RIGHTS.map((r) => ({
                    value: r,
                    label: t(`dialogs.taskMembers.right.${r}`),
                  }))}
                  onChange={(r) => changeRight(s, r)}
                  disabled={busy}
                />
              )}
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={`${t('dialogs.taskMembers.remove')}: ${s.user.name}`}
                disabled={busy}
                onPress={() => remove(s)}
                style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
              >
                <Text style={styles.removeButtonText} importantForAccessibility="no">
                  {t('dialogs.taskMembers.remove')}
                </Text>
              </Pressable>
            </View>
          ))}
        </View>
      )}

      <View style={styles.addSection}>
        <Text style={styles.label}>{t('dialogs.taskMembers.addLabel')}</Text>
        <View style={styles.addRow}>
          <TextInput
            style={styles.input}
            value={query}
            onChangeText={setQuery}
            placeholder={t(
              addByEmail
                ? 'dialogs.taskMembers.emailPlaceholder'
                : 'dialogs.taskMembers.searchPlaceholder',
            )}
            accessibilityLabel={t('dialogs.taskMembers.addLabel')}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType={addByEmail ? 'email-address' : 'default'}
            returnKeyType={addByEmail ? 'done' : 'search'}
            onSubmitEditing={addByEmail ? () => invite() : undefined}
          />
          {addByEmail && (
            <Pressable
              accessibilityRole="button"
              accessibilityState={{ disabled: busy || query.trim().length === 0 }}
              accessibilityLabel={t('dialogs.taskMembers.invite')}
              disabled={busy || query.trim().length === 0}
              onPress={() => invite()}
              style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
            >
              <Text style={styles.addButtonText}>{t('dialogs.taskMembers.invite')}</Text>
            </Pressable>
          )}
        </View>

        {/* Search-mode: a never-silent status line + the addable results. */}
        {!addByEmail && (
          <>
            {searchStatus !== '' && (
              <Text
                style={searchStatusError ? styles.error : styles.muted}
                accessibilityRole="text"
                accessibilityLiveRegion="polite"
              >
                {searchStatus}
              </Text>
            )}
            {addable.length > 0 && (
              <View accessibilityRole="list" style={styles.list}>
                {addable.map((u) => (
                  <Pressable
                    key={u.id}
                    accessibilityRole="button"
                    accessibilityState={{ disabled: busy }}
                    accessibilityLabel={u.email ? `${u.name} · ${u.email}` : u.name}
                    disabled={busy}
                    onPress={() => add(u)}
                    style={({ pressed }) => [styles.resultButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.resultText} importantForAccessibility="no">
                      {u.email ? `${u.name} · ${u.email}` : u.name}
                    </Text>
                  </Pressable>
                ))}
              </View>
            )}
          </>
        )}
      </View>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    muted: { fontSize: 15, color: c.textSecondary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    list: { gap: 10 },
    shareRow: {
      gap: 10,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    shareName: { fontSize: 17, fontWeight: '600', color: c.textPrimary },
    addSection: { gap: 10 },
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
    resultButton: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    resultText: { fontSize: 16, color: c.link, fontWeight: '600' },
    removeButton: {
      alignSelf: 'flex-start',
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    removeButtonText: { fontSize: 14, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
