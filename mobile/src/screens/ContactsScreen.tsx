import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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

import {
  Contact,
  ContactList,
  deleteContact as apiDeleteContact,
  getContacts,
  listContactLists,
  searchContacts,
} from '../api/contacts';
import type { RootStackScreenProps } from '../navigation/types';
import { useCacheReload } from '../state/cacheObserver';
import { useThemedStyles, type ThemeColors } from '../theme';

// Accessible address-book view — a linear, screen-reader-first list of every
// contact across all address books (local + external providers), grouped under
// each book's name, with create / edit / delete. Browsing lists the loaded
// contacts grouped by book; searching runs a real cross-account Host search
// (local FTS + each provider's search, incl. directories like the GAL) so a hit
// in an account whose page wasn't loaded — or a directory-only contact — still
// surfaces. Contacts read/write through the Host's on-device adapters; they are
// NOT on the sync event log.

const SEARCH_DEBOUNCE_MS = 250;

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** A contact paired with its owning list, for grouped rendering. */
interface Group {
  list: ContactList;
  contacts: Contact[];
}

export default function ContactsScreen({
  navigation,
}: RootStackScreenProps<'Contacts'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  // Host search results (flat Contact[]) while a query is active; null = not
  // searching (browse the loaded groups).
  const [searchResults, setSearchResults] = useState<Contact[] | null>(null);
  // Bumped to re-run an active search after a mutation (delete) or on focus
  // (the editor returned) — the search effect keys on it, so the overlay never
  // shows a stale/deleted row.
  const [refreshNonce, setRefreshNonce] = useState(0);

  const trimmedQuery = query.trim();
  const searching = trimmedQuery.length > 0;

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // Debounced cross-account Host search with a request-token stale guard (the
  // latest query wins). Empty query → clear results + browse the loaded groups.
  const searchToken = useRef(0);
  useEffect(() => {
    const token = (searchToken.current += 1);
    if (trimmedQuery === '') {
      setSearchResults(null);
      return;
    }
    const handle = setTimeout(() => {
      void (async () => {
        try {
          const hits = await searchContacts(trimmedQuery);
          if (searchToken.current === token) setSearchResults(hits);
        } catch {
          if (searchToken.current === token) setSearchResults([]);
        }
      })();
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [trimmedQuery, refreshNonce]);

  // The book name for a contact's owning list (from the loaded lists), or its
  // raw list id as a last resort (a directory contact's book may not be loaded).
  const bookName = useCallback(
    (listId: string) => groups.find((g) => g.list.id === listId)?.list.name ?? listId,
    [groups],
  );

  // What the list renders: the loaded groups when browsing, else the Host search
  // results grouped by their owning book (search supersets the loaded set, incl.
  // directories), so the grouped renderer is shared.
  const displayGroups = useMemo<Group[]>(() => {
    if (!searching || searchResults == null) return groups;
    const byList = new Map<string, Contact[]>();
    for (const c of searchResults) {
      const arr = byList.get(c.list_id);
      if (arr) arr.push(c);
      else byList.set(c.list_id, [c]);
    }
    return Array.from(byList.entries()).map(([listId, contacts]) => ({
      list:
        groups.find((g) => g.list.id === listId)?.list ??
        ({
          id: listId,
          name: bookName(listId),
          color: null,
          color_label: null,
          read_only: true,
          account_id: '',
        } as ContactList),
      contacts,
    }));
  }, [bookName, groups, searchResults, searching]);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // listContactLists also primes the Host's route map, so it must run
      // before getContacts (which routes by list id).
      const lists = await listContactLists();
      const withContacts = await Promise.all(
        lists.map(async (list) => ({
          list,
          contacts: await getContacts(list.id).catch(() => []),
        })),
      );
      setGroups(withContacts);
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setLoading(false);
    }
  }, [announce, t]);

  // Reload on mount + whenever the screen regains focus (after the editor). On
  // focus also re-run any active search so an edit made in the editor is
  // reflected in the overlay, not just the browse groups.
  useEffect(() => {
    const onFocus = () => {
      void load();
      setRefreshNonce((n) => n + 1);
    };
    const unsubscribe = navigation.addListener('focus', onFocus);
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update the browse groups while focused when an external contact-cache
  // refresh lands (the root observer already announced it politely). An active
  // search re-runs on focus, not here — the overlay is a transient view.
  useCacheReload('contacts', load);

  // First writable (local-or-not, not read-only) book is the create target.
  const writableListId =
    groups.find((g) => !g.list.read_only)?.list.id ?? groups[0]?.list.id ?? null;

  const addContact = useCallback(() => {
    if (writableListId == null) return;
    navigation.navigate('ContactEditor', {
      contactId: null,
      listId: writableListId,
    });
  }, [navigation, writableListId]);

  const editContact = useCallback(
    (c: Contact) =>
      navigation.navigate('ContactEditor', {
        contactId: c.id,
        listId: c.list_id,
      }),
    [navigation],
  );

  const removeContact = useCallback(
    (c: Contact) => {
      Alert.alert(
        t('dialogs.contact.deleteTitle'),
        t('dialogs.contact.deleteMessage', { name: c.display_name }),
        [
          { text: t('mobile.cancel'), style: 'cancel' },
          {
            text: t('dialogs.contact.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                try {
                  await apiDeleteContact(c.id, c.list_id);
                  announce(t('dialogs.contact.deleted', { name: c.display_name }));
                  await load();
                  // Re-run any active search so the deleted row leaves the
                  // overlay (load() only refreshes the browse groups).
                  setRefreshNonce((n) => n + 1);
                } catch (err) {
                  const message = errorMessage(err);
                  setError(message);
                  announce(t('mobile.error', { message }));
                }
              })();
            },
          },
        ],
      );
    },
    [announce, load, t],
  );

  /** Subtitle: organization, else the first email/phone — context without bloat. */
  const subtitle = (c: Contact): string =>
    c.organization ?? c.emails[0] ?? c.phone_numbers[0] ?? '';

  const total = groups.reduce((n, g) => n + g.contacts.length, 0);
  // Search is in flight until the debounced call resolves (results still null).
  const searchPending = searching && searchResults == null;
  const searchTotal = searchResults?.length ?? 0;

  return (
    <View style={styles.screen}>
      <View style={styles.actionBar}>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: writableListId == null }}
          accessibilityLabel={t('dialogs.contact.createTitle')}
          disabled={writableListId == null}
          onPress={addContact}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.primaryPressed,
            writableListId == null && styles.primaryDisabled,
          ]}
        >
          <Text style={styles.primaryButtonText}>{t('dialogs.contact.createTitle')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.manageContactLists')}
          onPress={() => navigation.navigate('ContactLists')}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>{t('mobile.manageContactLists')}</Text>
        </Pressable>
      </View>

      {/* Search is a SUPERSET of browse (local FTS + every provider, incl.
          directories), so the bar shows whenever any book exists — even if
          browse loaded zero contacts (a throttled directory, a transient error).*/}
      {!loading && groups.length > 0 && (
        <View style={styles.searchBar}>
          <TextInput
            style={styles.searchInput}
            value={query}
            onChangeText={setQuery}
            placeholder={t('views.contacts.searchPlaceholder')}
            accessibilityLabel={t('views.contacts.searchLabel')}
            autoCapitalize="none"
            autoCorrect={false}
            clearButtonMode="while-editing"
          />
          {/* Announce "searching"/"N results" — but NOT "0 results" (the empty
              placeholder below is the single polite announcer for no hits). */}
          {searching && (searchPending || searchTotal > 0) && (
            <Text
              style={styles.searchCount}
              accessibilityRole="text"
              accessibilityLiveRegion="polite"
            >
              {searchPending
                ? t('views.contacts.searching')
                : t('views.contacts.searchResults', { count: searchTotal })}
            </Text>
          )}
        </View>
      )}

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('mobile.loading')}>
          {t('mobile.loading')}
        </Text>
      ) : !searching && total === 0 ? (
        <Text style={styles.muted}>{t('mobile.noContacts')}</Text>
      ) : searching && !searchPending && searchTotal === 0 ? (
        <Text
          style={styles.muted}
          accessibilityRole="text"
          accessibilityLiveRegion="polite"
        >
          {t('views.contacts.searchEmpty', { query: query.trim() })}
        </Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {displayGroups.map((g) => (
            <View key={g.list.id} style={styles.group}>
              <Text style={styles.groupHeading} accessibilityRole="header">
                {g.list.name}
              </Text>
              {g.contacts.length === 0 ? (
                <Text style={styles.muted}>{t('mobile.noContacts')}</Text>
              ) : (
                g.contacts.map((c) => (
                  <View
                    key={c.id}
                    accessible
                    accessibilityRole="button"
                    accessibilityLabel={
                      subtitle(c) ? `${c.display_name}, ${subtitle(c)}` : c.display_name
                    }
                    accessibilityHint={t('mobile.contactHint')}
                    accessibilityActions={
                      g.list.read_only
                        ? [{ name: 'activate', label: t('dialogs.contact.editTitle') }]
                        : [
                            { name: 'activate', label: t('dialogs.contact.editTitle') },
                            { name: 'delete', label: t('dialogs.contact.delete') },
                          ]
                    }
                    onAccessibilityAction={(e) => {
                      if (e.nativeEvent.actionName === 'delete' && !g.list.read_only)
                        removeContact(c);
                      else editContact(c);
                    }}
                    style={styles.row}
                  >
                    <Pressable
                      accessible={false}
                      onPress={() => editContact(c)}
                      style={styles.rowText}
                    >
                      <Text style={styles.contactName}>{c.display_name}</Text>
                      {subtitle(c) !== '' && (
                        <Text style={styles.contactSub}>{subtitle(c)}</Text>
                      )}
                    </Pressable>
                    {/* Read-only book (a directory / GAL): view-only — no delete. */}
                    {!g.list.read_only && (
                      <Pressable
                        accessibilityRole="button"
                        accessibilityLabel={`${t('dialogs.contact.delete')}: ${c.display_name}`}
                        onPress={() => removeContact(c)}
                        style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
                      >
                        <Text style={styles.deleteButtonText}>{t('dialogs.contact.delete')}</Text>
                      </Pressable>
                    )}
                  </View>
                ))
              )}
            </View>
          ))}
        </ScrollView>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    actionBar: { flexDirection: 'row', gap: 10, padding: 12, alignItems: 'center' },
    searchBar: { paddingHorizontal: 12, paddingBottom: 8, gap: 6 },
    searchInput: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    searchCount: { fontSize: 13, color: c.textSecondary },
    primaryButton: {
      flex: 1,
      paddingVertical: 12,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    list: { gap: 18, padding: 16 },
    group: { gap: 10 },
    groupHeading: { fontSize: 16, fontWeight: '700', color: c.textLabel },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowText: { flex: 1, gap: 2 },
    contactName: { fontSize: 18, fontWeight: '600', color: c.textPrimary },
    contactSub: { fontSize: 14, color: c.textSecondary },
    deleteButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    deleteButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
