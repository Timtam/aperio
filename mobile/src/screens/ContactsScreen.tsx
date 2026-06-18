import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import {
  Contact,
  ContactList,
  deleteContact as apiDeleteContact,
  getContacts,
  listContactLists,
} from '../api/contacts';
import type { RootStackScreenProps } from '../navigation/types';

// Accessible address-book view — a linear, screen-reader-first list of every
// contact across all address books (local + external providers), grouped under
// each book's name, with create / edit / delete. Contacts read/write through
// the Host's on-device adapters; they are NOT on the sync event log.

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

  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

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

  // Reload on mount + whenever the screen regains focus (after the editor).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

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
      </View>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('mobile.loading')}>
          {t('mobile.loading')}
        </Text>
      ) : total === 0 ? (
        <Text style={styles.muted}>{t('mobile.noContacts')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {groups.map((g) => (
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
                    accessibilityActions={[
                      { name: 'activate', label: t('dialogs.contact.editTitle') },
                      { name: 'delete', label: t('dialogs.contact.delete') },
                    ]}
                    onAccessibilityAction={(e) => {
                      if (e.nativeEvent.actionName === 'delete') removeContact(c);
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
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={`${t('dialogs.contact.delete')}: ${c.display_name}`}
                      onPress={() => removeContact(c)}
                      style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.deleteButtonText}>{t('dialogs.contact.delete')}</Text>
                    </Pressable>
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

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  actionBar: { flexDirection: 'row', gap: 10, padding: 12, alignItems: 'center' },
  primaryButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  primaryPressed: { backgroundColor: '#1740a8' },
  primaryDisabled: { backgroundColor: '#9aa9c9' },
  primaryButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  list: { gap: 18, padding: 16 },
  group: { gap: 10 },
  groupHeading: { fontSize: 16, fontWeight: '700', color: '#2b3240' },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowText: { flex: 1, gap: 2 },
  contactName: { fontSize: 18, fontWeight: '600', color: '#10131a' },
  contactSub: { fontSize: 14, color: '#5b6573' },
  deleteButton: {
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#d9b3b0',
    backgroundColor: '#fbeceb',
  },
  deleteButtonText: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  pressed: { opacity: 0.7 },
  muted: { fontSize: 15, color: '#5b6573', padding: 16 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318', paddingHorizontal: 16 },
});
