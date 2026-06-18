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

import type { ColorLabel } from '@aperio/shared';

import { listColorLabels } from '../api/colorLabels';
import { renameContainer, setContainerColorLabel } from '../api/containerColor';
import {
  ContactList,
  createContactList,
  deleteContactList,
  listContactLists,
} from '../api/contacts';
import { ColorLabelSelect } from '../components/ColorLabelSelect';
import type { RootStackScreenProps } from '../navigation/types';

// Address-book management — create, rename, recolour, and delete address books.
// The mobile parallel of the Tasks ListsScreen, reached from ContactsScreen
// (which shows all books' contacts but offers no book-level management).
// Screen-reader-first: a name field + Add, then a list of books. "Edit" opens an
// in-place panel — a name field (writable books only; rename writes to the
// source) + a colour picker (ALL books, even read-only/provider ones: a contact
// list's colour is a host-local override, not a provider write). Delete is
// writable-only. Create makes a LOCAL book (the bridge takes the name only).

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function ContactListsScreen({
  navigation,
}: RootStackScreenProps<'ContactLists'>) {
  const { t } = useTranslation();

  const [lists, setLists] = useState<ContactList[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [newName, setNewName] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // The book being edited in place (id) + its draft name + draft colour-label id
  // ('' = no colour).
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const [editColor, setEditColor] = useState('');

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
      const [books, labels] = await Promise.all([
        listContactLists(),
        listColorLabels().catch(() => [] as ColorLabel[]),
      ]);
      setLists(books);
      setColorLabels(labels);
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

  // After a create/edit, move screen-reader focus to the affected row once the
  // refreshed list re-renders.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [lists]);

  /** The book's resolved colour hex (bound label's live hex, else native), or
   *  undefined — a swatch for sighted users. */
  const colorHex = useCallback(
    (book: ContactList): string | undefined => {
      if (book.color_label) {
        const label = colorLabels.find((l) => l.id === book.color_label);
        if (label) return label.hex;
      }
      return book.color?.hex ?? undefined;
    },
    [colorLabels],
  );

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

  const startEdit = useCallback((book: ContactList) => {
    setEditingId(book.id);
    setEditName(book.name);
    setEditColor(book.color_label ?? '');
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditName('');
    setEditColor('');
  }, []);

  const saveEdit = useCallback(
    async (book: ContactList) => {
      const wantName = editName.trim();
      // Rename only applies to writable books (it writes to the source); a colour
      // change applies to any book (a host-local override).
      const renaming = !book.read_only && wantName !== book.name;
      if (renaming && wantName.length === 0) {
        setError(t('sidebar.contactListNameRequired'));
        announce(t('sidebar.contactListNameRequired'));
        return;
      }
      const nextColor = editColor === '' ? null : editColor;
      const recolouring = nextColor !== (book.color_label ?? null);
      if (!renaming && !recolouring) {
        cancelEdit();
        return;
      }
      setError(null);
      try {
        if (renaming) await renameContainer(book.id, 'contact_list', wantName);
        if (recolouring) await setContainerColorLabel(book.id, 'contact_list', nextColor);
        setEditingId(null);
        pendingFocusId.current = book.id;
        // The colour picker already spoke its selection; only the rename needs an
        // explicit confirmation announce.
        if (renaming) announce(t('sidebar.contactListRenamed', { name: wantName }));
        await load();
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, cancelEdit, editColor, editName, load, t],
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
              <View key={book.id} style={styles.editPanel}>
                {!book.read_only && (
                  <TextInput
                    style={styles.editInput}
                    value={editName}
                    onChangeText={setEditName}
                    accessibilityLabel={t('mobile.rename')}
                    autoFocus
                    returnKeyType="done"
                    onSubmitEditing={() => void saveEdit(book)}
                  />
                )}
                <ColorLabelSelect
                  value={editColor}
                  labels={colorLabels}
                  onChange={setEditColor}
                />
                <View style={styles.editActions}>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('mobile.save')}
                    onPress={() => void saveEdit(book)}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>{t('mobile.save')}</Text>
                  </Pressable>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('mobile.cancel')}
                    onPress={cancelEdit}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>{t('mobile.cancel')}</Text>
                  </Pressable>
                </View>
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
                  {colorHex(book) != null && (
                    <View
                      accessible={false}
                      importantForAccessibility="no"
                      style={[styles.colorDot, { backgroundColor: colorHex(book) }]}
                    />
                  )}
                  <Text style={styles.bookName} importantForAccessibility="no">
                    {book.name}
                  </Text>
                </View>
                <Pressable
                  accessibilityRole="button"
                  accessibilityLabel={`${t('mobile.edit')}: ${book.name}`}
                  onPress={() => startEdit(book)}
                  style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                >
                  <Text style={styles.smallButtonText}>{t('mobile.edit')}</Text>
                </Pressable>
                {!book.read_only && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={`${t('mobile.delete')}: ${book.name}`}
                    onPress={() => removeBook(book)}
                    style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.deleteButtonText}>{t('mobile.delete')}</Text>
                  </Pressable>
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
  rowText: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 10 },
  bookName: { flex: 1, fontSize: 18, fontWeight: '600', color: '#10131a' },
  colorDot: {
    width: 14,
    height: 14,
    borderRadius: 7,
    borderWidth: 1,
    borderColor: 'rgba(0,0,0,0.18)',
  },
  editPanel: {
    gap: 10,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#1d4ed8',
    backgroundColor: '#f8fafc',
  },
  editInput: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#ffffff',
  },
  editActions: { flexDirection: 'row', gap: 10 },
  smallButton: {
    paddingVertical: 10,
    paddingHorizontal: 14,
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
