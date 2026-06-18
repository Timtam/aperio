import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { renameContainer } from '../api/containerColor';
import {
  ContactList,
  createContactList,
  deleteContactList,
  listContactLists,
} from '../api/contacts';
import type { RootStackScreenProps } from '../navigation/types';

// Address-book management — create a (local) address book and delete an existing
// one. The mobile parallel of the Tasks ListsScreen, reached from ContactsScreen
// (which itself shows all books' contacts but offers no book-level management).
// Screen-reader-first: a name field + Add, then a list of books; a writable book
// carries a context-labelled Delete (read-only/provider books are informational
// — no delete affordance). Create always makes a LOCAL book (the bridge takes
// the name only); rename + colour are deferred with the host-local overrides /
// the rename_contact_list bridge.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function ContactListsScreen({
  navigation,
}: RootStackScreenProps<'ContactLists'>) {
  const { t } = useTranslation();

  const [lists, setLists] = useState<ContactList[]>([]);
  const [newName, setNewName] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // The book currently being renamed in place (id) + its draft name.
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setLists(await listContactLists());
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setLoading(false);
    }
  }, [announce, t]);

  // Reload on mount + whenever the screen regains focus.
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // After a create, move screen-reader focus to the new row once the refreshed
  // list re-renders.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [lists]);

  const addBook = useCallback(async () => {
    const name = newName.trim();
    if (name.length === 0) {
      setError(t('sidebar.contactListNameRequired'));
      announce(t('sidebar.contactListNameRequired'));
      return;
    }
    setError(null);
    try {
      const created = await createContactList(name);
      setNewName('');
      pendingFocusId.current = created.id;
      announce(t('sidebar.contactListCreated', { name }));
      await load();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    }
  }, [announce, load, newName, t]);

  const startRename = useCallback((book: ContactList) => {
    setEditingId(book.id);
    setEditName(book.name);
  }, []);

  const cancelRename = useCallback(() => {
    setEditingId(null);
    setEditName('');
  }, []);

  const saveRename = useCallback(
    async (book: ContactList) => {
      const name = editName.trim();
      if (name.length === 0) {
        setError(t('sidebar.contactListNameRequired'));
        announce(t('sidebar.contactListNameRequired'));
        return;
      }
      setError(null);
      try {
        await renameContainer(book.id, 'contact_list', name);
        setEditingId(null);
        setEditName('');
        pendingFocusId.current = book.id;
        announce(t('sidebar.contactListRenamed', { name }));
        await load();
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, editName, load, t],
  );

  const removeBook = useCallback(
    (book: ContactList) => {
      Alert.alert(
        t('dialogs.confirm.deleteContactListTitle'),
        t('dialogs.confirm.deleteContactListMessage', { name: book.name }),
        [
          { text: t('mobile.cancel'), style: 'cancel' },
          {
            text: t('mobile.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                try {
                  await deleteContactList(book.id);
                  announce(t('sidebar.contactListDeleted', { name: book.name }));
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

  return (
    <View style={styles.screen}>
      <View style={styles.form}>
        <TextInput
          style={styles.input}
          value={newName}
          onChangeText={setNewName}
          placeholder={t('sidebar.newContactList')}
          accessibilityLabel={t('sidebar.newContactList')}
          returnKeyType="done"
          onSubmitEditing={() => void addBook()}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.add')}
          onPress={() => void addBook()}
          style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
        >
          <Text style={styles.buttonText}>{t('mobile.add')}</Text>
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
      ) : lists.length === 0 ? (
        <Text style={styles.muted}>{t('mobile.noContacts')}</Text>
      ) : (
        <ScrollView
          accessibilityRole="list"
          contentContainerStyle={styles.list}
          keyboardShouldPersistTaps="handled"
        >
          {lists.map((book) =>
            editingId === book.id ? (
              <View key={book.id} style={styles.row}>
                <TextInput
                  style={styles.editInput}
                  value={editName}
                  onChangeText={setEditName}
                  accessibilityLabel={t('mobile.rename')}
                  autoFocus
                  returnKeyType="done"
                  onSubmitEditing={() => void saveRename(book)}
                />
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={t('mobile.save')}
                  onPress={() => void saveRename(book)}
                  style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                >
                  <Text style={styles.smallButtonText}>{t('mobile.save')}</Text>
                </Pressable>
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={t('mobile.cancel')}
                  onPress={cancelRename}
                  style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                >
                  <Text style={styles.smallButtonText}>{t('mobile.cancel')}</Text>
                </Pressable>
              </View>
            ) : (
              <View key={book.id} style={styles.row}>
                <View
                  ref={(node) => {
                    rowTags.current[book.id] = node ? findNodeHandle(node) : null;
                  }}
                  accessible
                  accessibilityRole="text"
                  accessibilityLabel={book.name}
                  style={styles.rowText}
                >
                  <Text style={styles.bookName} importantForAccessibility="no">
                    {book.name}
                  </Text>
                </View>
                {!book.read_only && (
                  <>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={`${t('mobile.rename')}: ${book.name}`}
                      onPress={() => startRename(book)}
                      style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.smallButtonText}>{t('mobile.rename')}</Text>
                    </Pressable>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={`${t('mobile.delete')}: ${book.name}`}
                      onPress={() => removeBook(book)}
                      style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.deleteButtonText}>{t('mobile.delete')}</Text>
                    </Pressable>
                  </>
                )}
              </View>
            ),
          )}
        </ScrollView>
      )}
    </View>
  );
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
    gap: 12,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowText: { flex: 1 },
  bookName: { fontSize: 18, fontWeight: '600', color: '#10131a' },
  editInput: {
    flex: 1,
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#1d4ed8',
    backgroundColor: '#ffffff',
  },
  smallButton: {
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  smallButtonText: { fontSize: 15, fontWeight: '600', color: '#1d4ed8' },
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
