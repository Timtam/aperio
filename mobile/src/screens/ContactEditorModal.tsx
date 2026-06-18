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

import {
  Contact,
  ContactList,
  createContact,
  getContacts,
  listContactLists,
  updateContact,
} from '../api/contacts';
import { RadioGroup } from '../components/RadioGroup';
import type { RootStackScreenProps } from '../navigation/types';

// Create / edit a contact. Screen-reader-first: every field is a labelled stop;
// the address book is a radio group (no native select); emails + phone numbers
// are comma-separated single fields (matching the desktop ContactDialog), which
// is far less fiddly for a keyboard/SR user than per-value add/remove rows. The
// rich fields (postal addresses, photo, group members, birthday) round-trip
// untouched on edit and are deferred from this first editor.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Split a comma-separated field into trimmed, non-empty values. */
function splitList(raw: string): string[] {
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export default function ContactEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'ContactEditor'>) {
  const { t } = useTranslation();
  const { contactId, listId } = route.params;
  const editing = contactId != null;

  const [lists, setLists] = useState<ContactList[]>([]);
  const [selectedListId, setSelectedListId] = useState(listId);
  const [displayName, setDisplayName] = useState('');
  const [givenName, setGivenName] = useState('');
  const [familyName, setFamilyName] = useState('');
  const [organization, setOrganization] = useState('');
  const [emailsText, setEmailsText] = useState('');
  const [phonesText, setPhonesText] = useState('');
  const [notes, setNotes] = useState('');
  /** The loaded contact (edit mode) — kept whole so un-edited fields
   *  (addresses, members, photo, etag, birthday, timestamps) round-trip. */
  const [original, setOriginal] = useState<Contact | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const all = await listContactLists();
        if (cancelled) return;
        // Writable books are the create/move targets; always include the
        // contact's current book even if read-only so editing still works.
        setLists(all.filter((l) => !l.read_only || l.id === listId));
        if (editing) {
          const found = (await getContacts(listId)).find((c) => c.id === contactId);
          if (cancelled || !found) return;
          setOriginal(found);
          setDisplayName(found.display_name);
          setGivenName(found.given_name ?? '');
          setFamilyName(found.family_name ?? '');
          setOrganization(found.organization ?? '');
          setEmailsText(found.emails.join(', '));
          setPhonesText(found.phone_numbers.join(', '));
          setNotes(found.notes ?? '');
          setSelectedListId(found.list_id);
        }
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [contactId, editing, listId]);

  const listOptions = useMemo(
    () => lists.map((l) => ({ value: l.id, label: l.name })),
    [lists],
  );

  const save = useCallback(async () => {
    const name = displayName.trim();
    if (name.length === 0) {
      const message = t('dialogs.contact.displayNameRequired');
      setError(message);
      announce(message);
      return;
    }
    setBusy(true);
    setError(null);
    const given = givenName.trim() || null;
    const family = familyName.trim() || null;
    const org = organization.trim() || null;
    const note = notes.trim() || null;
    const emails = splitList(emailsText);
    const phones = splitList(phonesText);
    try {
      if (editing && original) {
        await updateContact({
          ...original,
          list_id: selectedListId,
          display_name: name,
          given_name: given,
          family_name: family,
          organization: org,
          emails,
          phone_numbers: phones,
          notes: note,
        });
      } else {
        await createContact(selectedListId, {
          display_name: name,
          given_name: given,
          family_name: family,
          organization: org,
          emails,
          phone_numbers: phones,
          birthday: null,
          notes: note,
          addresses: [],
          members: null,
          photo: null,
        });
      }
      announce(t('mobile.saved', { title: name }));
      navigation.goBack();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [
    announce,
    displayName,
    editing,
    emailsText,
    familyName,
    givenName,
    navigation,
    notes,
    organization,
    original,
    phonesText,
    selectedListId,
    t,
  ]);

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

      <Field label={t('dialogs.contact.displayNameLabel')}>
        <TextInput
          style={styles.input}
          value={displayName}
          onChangeText={setDisplayName}
          accessibilityLabel={t('dialogs.contact.displayNameLabel')}
        />
      </Field>

      <Field label={t('dialogs.contact.givenNameLabel')}>
        <TextInput
          style={styles.input}
          value={givenName}
          onChangeText={setGivenName}
          accessibilityLabel={t('dialogs.contact.givenNameLabel')}
        />
      </Field>

      <Field label={t('dialogs.contact.familyNameLabel')}>
        <TextInput
          style={styles.input}
          value={familyName}
          onChangeText={setFamilyName}
          accessibilityLabel={t('dialogs.contact.familyNameLabel')}
        />
      </Field>

      <Field label={t('dialogs.contact.organizationLabel')}>
        <TextInput
          style={styles.input}
          value={organization}
          onChangeText={setOrganization}
          accessibilityLabel={t('dialogs.contact.organizationLabel')}
        />
      </Field>

      <Field label={t('dialogs.contact.emailsLabel')} hint={t('dialogs.contact.emailsHint')}>
        <TextInput
          style={styles.input}
          value={emailsText}
          onChangeText={setEmailsText}
          placeholder={t('dialogs.contact.emailsPlaceholder')}
          accessibilityLabel={t('dialogs.contact.emailsLabel')}
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="email-address"
        />
      </Field>

      <Field
        label={t('dialogs.contact.phoneNumbersLabel')}
        hint={t('dialogs.contact.phoneNumbersHint')}
      >
        <TextInput
          style={styles.input}
          value={phonesText}
          onChangeText={setPhonesText}
          placeholder={t('dialogs.contact.phoneNumbersPlaceholder')}
          accessibilityLabel={t('dialogs.contact.phoneNumbersLabel')}
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="phone-pad"
        />
      </Field>

      <Field label={t('dialogs.contact.notesLabel')}>
        <TextInput
          style={[styles.input, styles.multiline]}
          value={notes}
          onChangeText={setNotes}
          accessibilityLabel={t('dialogs.contact.notesLabel')}
          multiline
        />
      </Field>

      {listOptions.length > 0 && (
        <RadioGroup
          label={t('dialogs.contact.listLabel')}
          value={selectedListId}
          options={listOptions}
          onChange={setSelectedListId}
          disabled={busy}
        />
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: busy }}
        accessibilityLabel={t('mobile.save')}
        disabled={busy}
        onPress={() => void save()}
        style={({ pressed }) => [
          styles.primaryButton,
          pressed && styles.primaryPressed,
          busy && styles.primaryDisabled,
        ]}
      >
        <Text style={styles.primaryButtonText}>{t('mobile.save')}</Text>
      </Pressable>
    </ScrollView>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <View style={styles.field}>
      <Text style={styles.label}>{label}</Text>
      {hint != null && (
        <Text style={styles.hint} accessibilityRole="text">
          {hint}
        </Text>
      )}
      {children}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 16 },
  field: { gap: 6 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  hint: { fontSize: 13, color: '#5b6573' },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  multiline: { minHeight: 88, textAlignVertical: 'top' },
  primaryButton: {
    paddingVertical: 14,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  primaryPressed: { backgroundColor: '#1740a8' },
  primaryDisabled: { backgroundColor: '#9aa9c9' },
  primaryButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
});
