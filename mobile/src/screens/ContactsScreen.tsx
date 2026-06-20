import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  SectionList,
  type SectionListData,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  Contact,
  ContactList,
  ContactsSyncStatus,
  deleteContact as apiDeleteContact,
  getContacts,
  getContactsSyncStatus,
  listContactLists,
  searchContacts,
} from '../api/contacts';
import { useTabBarInset } from '../hooks/useTabBarInset';
import type { RootStackScreenProps } from '../navigation/types';
import { useCacheReload } from '../state/cacheObserver';
import { useContactVisibility } from '../state/contactVisibility';
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
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const tabBarInset = useTabBarInset();

  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  // Contact-sync status (for the browse-surface "Sync now" + last-synced line —
  // the desktop surfaces a manual refresh here, not only in Settings).
  const [syncStatus, setSyncStatus] = useState<ContactsSyncStatus | null>(null);
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

  // Per-device address-book visibility — the user can hide books from browse +
  // search (toggled on the Manage-address-books screen).
  const { hidden: hiddenBooks } = useContactVisibility();

  // What the list renders: the loaded groups when browsing, else the Host search
  // results grouped by their owning book (search supersets the loaded set, incl.
  // directories), so the grouped renderer is shared. Hidden books drop from both.
  const displayGroups = useMemo<Group[]>(() => {
    let base: Group[];
    if (!searching || searchResults == null) {
      base = groups;
    } else {
      const byList = new Map<string, Contact[]>();
      for (const c of searchResults) {
        const arr = byList.get(c.list_id);
        if (arr) arr.push(c);
        else byList.set(c.list_id, [c]);
      }
      base = Array.from(byList.entries()).map(([listId, contacts]) => ({
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
    }
    // Order: local/personal books first, then editable external books, then
    // read-only directories (an EWS global address list can be thousands of
    // contacts). A virtualized SectionList can't rotor-jump to a header that
    // isn't rendered, so burying the small personal books under a giant
    // directory made them unreachable by heading — keep the important books at
    // the top and sink the huge directory to the end. Stable sort preserves the
    // host's order within each rank.
    const rank = (g: Group) =>
      g.list.account_id === 'local' ? 0 : g.list.read_only ? 2 : 1;
    return base
      .filter((g) => !hiddenBooks.has(g.list.id))
      .sort((a, b) => rank(a) - rank(b));
  }, [bookName, groups, searchResults, searching, hiddenBooks]);

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
      void getContactsSyncStatus().then(setSyncStatus).catch(() => {});
    };
    const unsubscribe = navigation.addListener('focus', onFocus);
    void load();
    void getContactsSyncStatus().then(setSyncStatus).catch(() => {});
    return unsubscribe;
  }, [navigation, load]);

  const lastSynced = useMemo(() => {
    if (!syncStatus?.last_synced_at) return t('dialogs.settings.contacts.neverSynced');
    const d = new Date(syncStatus.last_synced_at);
    if (Number.isNaN(d.getTime())) return t('dialogs.settings.contacts.neverSynced');
    const fmt = new Intl.DateTimeFormat(i18n.language, {
      dateStyle: 'long',
      timeStyle: 'short',
    });
    return t('dialogs.settings.contacts.lastSynced', { time: fmt.format(d) });
  }, [syncStatus?.last_synced_at, i18n.language, t]);

  // Live-update the browse groups while focused when an external contact-cache
  // refresh lands. A background contacts sync (or a cache warm) can stream MANY
  // cache-update events; debounce so the big (GAL-sized) list reloads once per
  // burst instead of re-fetching + re-rendering on every event.
  const reloadTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const reloadFromCache = useCallback(() => {
    if (reloadTimer.current != null) clearTimeout(reloadTimer.current);
    reloadTimer.current = setTimeout(() => {
      reloadTimer.current = null;
      void load();
    }, 250);
  }, [load]);
  useEffect(
    () => () => {
      if (reloadTimer.current != null) clearTimeout(reloadTimer.current);
    },
    [],
  );
  useCacheReload('contacts', reloadFromCache);

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

  /** Subtitle: a group's member count, else organization / first email / phone —
   *  context without bloat. */
  const subtitle = (c: Contact): string => {
    // A distribution list (group) carries `members`; show its size (a group has
    // no organization/email/phone), so the row reads as a group + its count.
    if (c.members != null) {
      return t('dialogs.contact.memberCount', { count: c.members.length });
    }
    return c.organization ?? c.emails[0] ?? c.phone_numbers[0] ?? '';
  };

  const total = displayGroups.reduce((n, g) => n + g.contacts.length, 0);
  // Whether any address book is actually syncable (an external provider). Local
  // device books never sync, so for an all-local setup the manual sync + the
  // last-synced line are meaningless and hidden.
  const hasSyncable = groups.some((g) => g.list.account_id !== 'local');
  // Search is in flight until the debounced call resolves (results still null).
  const searchPending = searching && searchResults == null;
  const searchTotal = searchResults?.length ?? 0;

  // Address books are collapsible and COLLAPSED BY DEFAULT (browse): you get a
  // compact list of book headers and expand only the one you want, so a giant
  // external directory doesn't bury the small personal books in the scroll view
  // (and the headings rotor reaches every book). Session-local; search always
  // shows its results expanded.
  const [expandedBooks, setExpandedBooks] = useState<Set<string>>(new Set());
  const toggleBook = useCallback((id: string) => {
    setExpandedBooks((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  // SectionList feed: one section per address book. A VIRTUALIZED list (cells
  // recycled) so an expanded directory with thousands of contacts (an EWS global
  // address list) scrolls smoothly; a collapsed section carries empty data so
  // only its header renders.
  const sections = useMemo<
    SectionListData<Contact, { list: ContactList; collapsed: boolean; count: number }>[]
  >(
    () =>
      displayGroups.map((g) => {
        const collapsed = !searching && !expandedBooks.has(g.list.id);
        return {
          list: g.list,
          collapsed,
          count: g.contacts.length,
          data: collapsed ? [] : g.contacts,
        };
      }),
    [displayGroups, searching, expandedBooks],
  );

  const renderContact = (c: Contact, list: ContactList) => (
    <View
      accessible
      accessibilityRole="button"
      accessibilityLabel={subtitle(c) ? `${c.display_name}, ${subtitle(c)}` : c.display_name}
      accessibilityHint={t('mobile.contactHint')}
      accessibilityActions={
        list.read_only
          ? [{ name: 'activate', label: t('dialogs.contact.editTitle') }]
          : [
              { name: 'activate', label: t('dialogs.contact.editTitle') },
              { name: 'delete', label: t('dialogs.contact.delete') },
            ]
      }
      onAccessibilityAction={(e) => {
        if (e.nativeEvent.actionName === 'delete' && !list.read_only) removeContact(c);
        else editContact(c);
      }}
      style={styles.row}
    >
      <Pressable accessible={false} onPress={() => editContact(c)} style={styles.rowText}>
        <Text style={styles.contactName}>{c.display_name}</Text>
        {subtitle(c) !== '' && <Text style={styles.contactSub}>{subtitle(c)}</Text>}
      </Pressable>
      {/* Read-only book (a directory / GAL): view-only — no delete. */}
      {!list.read_only && (
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
  );

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
        {/* Contact sync runs automatically (periodic + the header sync action);
            the manual control + full settings live under Settings → Contacts. */}
      </View>

      {/* Last-synced line only once a sync has actually recorded a time AND a
          syncable account exists — never the misleading "never synced" while
          contacts are already showing. */}
      {hasSyncable && syncStatus?.last_synced_at != null && (
        <Text style={styles.lastSynced} accessibilityRole="text">
          {lastSynced}
        </Text>
      )}

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
        <SectionList
          accessibilityRole="list"
          sections={sections}
          keyExtractor={(c) => c.id}
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
          stickySectionHeadersEnabled={false}
          initialNumToRender={20}
          windowSize={11}
          removeClippedSubviews
          renderSectionHeader={({ section }) => {
            const label = t('views.contacts.groupLabel', {
              name: section.list.name,
              count: section.count,
            });
            // Search shows its results — a plain header. Browsing, the header is
            // a heading (rotor-reachable) that toggles its book open/closed,
            // collapsed by default, announcing its expanded state.
            if (searching) {
              return (
                <Text style={styles.groupHeading} accessibilityRole="header">
                  {label}
                </Text>
              );
            }
            return (
              <Pressable
                accessible
                accessibilityRole="header"
                accessibilityLabel={label}
                accessibilityHint={t('mobile.groupHeaderHint')}
                accessibilityState={{ expanded: !section.collapsed }}
                onPress={() => toggleBook(section.list.id)}
                style={({ pressed }) => [styles.groupHeadingRow, pressed && styles.pressed]}
              >
                <Text style={styles.twisty} importantForAccessibility="no">
                  {section.collapsed ? '▸' : '▾'}
                </Text>
                <Text style={styles.groupHeading} importantForAccessibility="no">
                  {label}
                </Text>
              </Pressable>
            );
          }}
          renderSectionFooter={({ section }) =>
            !section.collapsed && section.count === 0 ? (
              <Text style={styles.muted}>{t('mobile.noContacts')}</Text>
            ) : null
          }
          renderItem={({ item, section }) => renderContact(item, section.list)}
        />
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    actionBar: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 10,
      padding: 12,
      alignItems: 'center',
    },
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
    lastSynced: { fontSize: 13, color: c.textSecondary, paddingHorizontal: 12, paddingBottom: 6 },
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
    // Browse: the collapsible book header (twisty + name); search reuses the
    // bare groupHeading text above.
    groupHeadingRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      paddingVertical: 10,
      paddingHorizontal: 12,
    },
    twisty: { fontSize: 16, width: 18, textAlign: 'center', color: c.textLabel },
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
