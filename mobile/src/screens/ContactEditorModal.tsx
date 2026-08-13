import * as ImagePicker from 'expo-image-picker';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Image,
  Pressable,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  Contact,
  ContactAddress,
  ContactList,
  ContactPhoto,
  createContact,
  deleteContactPhoto,
  getContactPhoto,
  getContacts,
  listContactLists,
  setContactPhoto,
  updateContact,
} from '../api/contacts';
import {
  fromContactValues,
  toContactValues,
  type ContactValue,
} from '@aperio/shared';

import { ContactLabelButton } from '../components/ContactLabelButton';
import { DateTimeFieldButton } from '../components/DateTimeFieldButton';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { formatLocalDate } from '../intl/dateTimeField';
import { useListFocusManager } from '../a11y/useListFocusManager';
import type { RootStackScreenProps } from '../navigation/types';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Create / edit a contact OR a distribution list (group). Screen-reader-first:
// every field is a labelled stop; the address book is a radio group (no native
// select); emails, phone numbers and websites are LABELLED rows — one value
// per row with its label on a button that opens the choice in a dialog, so a
// screen reader announces "mobile" before the digits and a contact with four
// numbers still takes four swipes, not two dozen. Postal addresses are the
// same dynamic-row shape (empty rows drop on save), plus birthday and
// anniversary, job title and department. A "distribution list" switch turns
// the person fields into a members editor (one "Name <email>" / bare email per
// line), exactly like the desktop. The avatar is shown (a real image for sighted
// users + an accessible alt), can be set from the device photo library (the
// image picker), and removed; on a new contact the picked photo rides the
// create.

/** Photo guards — match the desktop ContactDialog (MAX_PHOTO_BYTES /
 *  ALLOWED_PHOTO_TYPES). */
const MAX_PHOTO_BYTES = 5 * 1024 * 1024;
const ALLOWED_PHOTO_TYPES = ['image/jpeg', 'image/png', 'image/gif'];

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** One member of a distribution list (the cal_core GroupMember shape). */
interface GroupMember {
  name: string | null;
  email: string;
}

/** Parse the members textarea into structured records — one per line, either
 *  "Name <email>" (CN kept) or a bare email; lines without an "@" are skipped
 *  (a member needs an email). Ported verbatim from the desktop ContactDialog. */
function parseMembers(raw: string): GroupMember[] {
  const out: GroupMember[] = [];
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const angle = trimmed.match(/^(.*?)\s*<\s*([^>]+?)\s*>\s*$/);
    if (angle) {
      const email = angle[2].trim();
      if (email.includes('@')) out.push({ name: angle[1].trim() || null, email });
      continue;
    }
    if (trimmed.includes('@')) out.push({ name: null, email: trimmed });
  }
  return out;
}

/** Members → the editable textarea text ("Name <email>" / bare email per line). */
function formatMembers(members: unknown[] | null): string {
  if (members == null) return '';
  return members
    .map((m) => {
      const r = m as Partial<GroupMember>;
      return r.name ? `${r.name} <${r.email ?? ''}>` : (r.email ?? '');
    })
    .join('\n');
}

/** Editable address row — all fields are strings (`''` = empty) so they bind to
 *  TextInputs; mapped to/from the wire `ContactAddress` (null = empty). */
interface AddressRow {
  label: string;
  street: string;
  city: string;
  region: string;
  postal_code: string;
  country: string;
}

const EMPTY_ADDRESS: AddressRow = {
  label: '',
  street: '',
  city: '',
  region: '',
  postal_code: '',
  country: '',
};

/** Wire address → editable row (a `None`/absent field reads as `''`). */
function toRow(a: ContactAddress): AddressRow {
  return {
    label: a.label ?? '',
    street: a.street ?? '',
    city: a.city ?? '',
    region: a.region ?? '',
    postal_code: a.postal_code ?? '',
    country: a.country ?? '',
  };
}

/** Editable rows → wire addresses: trim, empty field → null, and drop a row
 *  whose every field is blank (matching the desktop `sanitiseAddresses`). */
function sanitiseAddresses(rows: AddressRow[]): ContactAddress[] {
  return rows
    .map((r) => ({
      label: r.label.trim() || null,
      street: r.street.trim() || null,
      city: r.city.trim() || null,
      region: r.region.trim() || null,
      postal_code: r.postal_code.trim() || null,
      country: r.country.trim() || null,
    }))
    .filter(
      (a) =>
        a.label !== null ||
        a.street !== null ||
        a.city !== null ||
        a.region !== null ||
        a.postal_code !== null ||
        a.country !== null,
    );
}

export default function ContactEditorModal({
  route,
  navigation,
}: RootStackScreenProps<'ContactEditor'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  const { contactId, listId } = route.params;
  const editing = contactId != null;

  const [lists, setLists] = useState<ContactList[]>([]);
  const [selectedListId, setSelectedListId] = useState(listId);
  const [displayName, setDisplayName] = useState('');
  const [givenName, setGivenName] = useState('');
  const [familyName, setFamilyName] = useState('');
  const [organization, setOrganization] = useState('');
  const [jobTitle, setJobTitle] = useState('');
  const [department, setDepartment] = useState('');
  const [emails, setEmails] = useState<ContactValue[]>([]);
  const [phones, setPhones] = useState<ContactValue[]>([]);
  const [urls, setUrls] = useState<ContactValue[]>([]);
  const [birthday, setBirthday] = useState('');
  const [anniversary, setAnniversary] = useState('');
  const [addresses, setAddresses] = useState<AddressRow[]>([]);
  const [notes, setNotes] = useState('');
  // A distribution list (group) vs a person; when on, the person fields give way
  // to a members editor. `members != null` marks a group on the wire.
  const [isGroup, setIsGroup] = useState(false);
  const [membersText, setMembersText] = useState('');
  // An existing contact's avatar (fetched when has_photo), or null. Display +
  // remove only — setting a new photo needs the device image picker (follow-up).
  const [photo, setPhoto] = useState<ContactPhoto | null>(null);
  const addressFocus = useListFocusManager(addresses.length);
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
          if (cancelled) return;
          // A miss (e.g. a directory hit the re-enumeration didn't return) must
          // NOT leave a silent blank form that would save as a junk create.
          if (!found) {
            setError(t('dialogs.contact.loadFailed'));
            return;
          }
          setOriginal(found);
          setDisplayName(found.display_name);
          setGivenName(found.given_name ?? '');
          setFamilyName(found.family_name ?? '');
          setOrganization(found.organization ?? '');
          setJobTitle(found.job_title ?? '');
          setDepartment(found.department ?? '');
          setEmails(toContactValues(found.emails));
          setPhones(toContactValues(found.phone_numbers));
          setUrls(toContactValues(found.urls));
          setBirthday(found.birthday ?? '');
          setAnniversary(found.anniversary ?? '');
          setAddresses((found.addresses ?? []).map(toRow));
          setNotes(found.notes ?? '');
          setIsGroup(found.members !== null);
          setMembersText(formatMembers(found.members));
          setSelectedListId(found.list_id);
          // Fetch the avatar lazily (only when the row says it has one). A load
          // failure is non-fatal — leave it null, the editor still works.
          if (found.has_photo) {
            try {
              const p = await getContactPhoto(found.id, found.list_id);
              if (!cancelled) setPhoto(p);
            } catch {
              /* photoLoadFailed — non-fatal; the section shows "no photo". */
            }
          }
        }
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [contactId, editing, listId, t]);

  const listOptions = useMemo(
    () => lists.map((l) => ({ value: l.id, label: l.name })),
    [lists],
  );

  // A read-only address book (e.g. a directory / GAL): the contact is view-only
  // — disable the structural controls + hide Save (the desktop ContactDialog
  // disables the whole form for read-only books).
  const viewOnly =
    editing && (lists.find((l) => l.id === selectedListId)?.read_only ?? false);

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
    const birthdayValue = birthday.trim() || null;
    // A group carries members (and no person fields); a person carries the
    // person fields and members: null. `members != null` is the wire marker.
    const members = isGroup ? parseMembers(membersText) : null;
    // A group carries none of the person-only channels. Emails DO travel on a
    // group in the desktop editor's model too — but a distribution list's
    // addresses are its members, so the channel lists go empty here.
    const cleanEmails = isGroup ? [] : fromContactValues(emails);
    const cleanPhones = isGroup ? [] : fromContactValues(phones);
    const cleanUrls = isGroup ? [] : fromContactValues(urls);
    const cleanedAddresses = isGroup ? [] : sanitiseAddresses(addresses);
    const personGiven = isGroup ? null : given;
    const personFamily = isGroup ? null : family;
    const personOrg = isGroup ? null : org;
    const personBirthday = isGroup ? null : birthdayValue;
    const personAnniversary = isGroup ? null : anniversary.trim() || null;
    const personJobTitle = isGroup ? null : jobTitle.trim() || null;
    const personDepartment = isGroup ? null : department.trim() || null;
    try {
      if (editing) {
        if (original == null) {
          // The contact never loaded (a missed directory hit) — refuse to save
          // (it would otherwise fall through to a junk create).
          setError(t('dialogs.contact.loadFailed'));
          return;
        }
        await updateContact({
          ...original,
          list_id: selectedListId,
          display_name: name,
          given_name: personGiven,
          family_name: personFamily,
          organization: personOrg,
          job_title: personJobTitle,
          department: personDepartment,
          emails: cleanEmails,
          phone_numbers: cleanPhones,
          urls: cleanUrls,
          birthday: personBirthday,
          anniversary: personAnniversary,
          addresses: cleanedAddresses,
          notes: note,
          members,
        });
      } else {
        await createContact(selectedListId, {
          display_name: name,
          given_name: personGiven,
          family_name: personFamily,
          organization: personOrg,
          job_title: personJobTitle,
          department: personDepartment,
          emails: cleanEmails,
          phone_numbers: cleanPhones,
          urls: cleanUrls,
          birthday: personBirthday,
          anniversary: personAnniversary,
          notes: note,
          addresses: cleanedAddresses,
          members,
          // A photo picked before saving rides the create (a person only — a
          // distribution list has no avatar).
          photo: isGroup ? null : photo,
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
    addresses,
    anniversary,
    announce,
    birthday,
    department,
    displayName,
    editing,
    emails,
    familyName,
    givenName,
    isGroup,
    jobTitle,
    membersText,
    navigation,
    notes,
    organization,
    original,
    phones,
    photo,
    selectedListId,
    t,
    urls,
  ]);

  const removePhoto = useCallback(async () => {
    // A new (unsaved) contact's picked photo lives only in state — just clear it.
    if (original == null) {
      setPhoto(null);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await deleteContactPhoto(original.id, original.list_id);
      setPhoto(null);
      setOriginal({ ...original, has_photo: false });
      announce(t('dialogs.contact.photoRemoved', { name: original.display_name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, original, t]);

  // Pick an image from the device library → validate type + size → set it. For
  // an existing contact it's pushed immediately (setContactPhoto); for a new one
  // it's held in state and rides the create. A cancel / denied permission is
  // silent.
  const pickPhoto = useCallback(async () => {
    const perm = await ImagePicker.requestMediaLibraryPermissionsAsync();
    if (!perm.granted) return;
    const result = await ImagePicker.launchImageLibraryAsync({
      mediaTypes: 'images',
      base64: true,
      quality: 0.7,
    });
    if (result.canceled) return;
    const asset = result.assets[0];
    const mime = asset.mimeType ?? 'image/jpeg';
    if (!ALLOWED_PHOTO_TYPES.includes(mime)) {
      const m = t('dialogs.contact.photoUnsupportedType');
      setError(m);
      announce(m);
      return;
    }
    if (asset.base64 == null) {
      const m = t('dialogs.contact.photoLoadFailed');
      setError(m);
      announce(m);
      return;
    }
    // base64 decodes to ~3/4 of its length in bytes.
    if (Math.floor((asset.base64.length * 3) / 4) > MAX_PHOTO_BYTES) {
      const m = t('dialogs.contact.photoTooLarge', {
        limit: `${MAX_PHOTO_BYTES / (1024 * 1024)} MB`,
      });
      setError(m);
      announce(m);
      return;
    }
    const picked: ContactPhoto = { content_type: mime, data: asset.base64 };
    if (original != null) {
      setBusy(true);
      setError(null);
      try {
        await setContactPhoto(original.id, picked, original.list_id);
        setPhoto(picked);
        setOriginal({ ...original, has_photo: true });
        announce(t('dialogs.contact.photoUpdated', { name: original.display_name }));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    } else {
      // New contact — hold it; it's saved with the create.
      setPhoto(picked);
      setError(null);
      announce(t('dialogs.contact.photoUpdated', { name: displayName.trim() }));
    }
  }, [announce, displayName, original, t]);

  const addAddress = useCallback(() => {
    addressFocus.onAdd();
    setAddresses((rows) => [...rows, { ...EMPTY_ADDRESS }]);
  }, [addressFocus]);

  const removeAddress = useCallback(
    (index: number) => {
      addressFocus.onRemove(index);
      setAddresses((rows) => rows.filter((_, i) => i !== index));
    },
    [addressFocus],
  );

  const updateAddress = useCallback(
    (index: number, field: keyof AddressRow, value: string) => {
      setAddresses((rows) =>
        rows.map((row, i) => (i === index ? { ...row, [field]: value } : row)),
      );
    },
    [],
  );

  return (
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {viewOnly && (
        <Text style={styles.readOnlyBanner} accessibilityRole="text">
          {t('dialogs.contact.readOnlyHint')}
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

      {/* Photo — a person's avatar (a distribution list has none). A real image
          for sighted users carrying an accessible alt; pick from the device
          library (new + existing) and remove. */}
      {!isGroup && (
        <View style={styles.field}>
          <Text style={styles.label} accessibilityRole="header">
            {t('dialogs.contact.photoSectionLabel')}
          </Text>
          {photo != null ? (
            <Image
              source={{ uri: `data:${photo.content_type};base64,${photo.data}` }}
              style={styles.photo}
              accessible
              accessibilityRole="image"
              accessibilityLabel={t('dialogs.contact.photoAltSet', {
                name: original?.display_name ?? displayName,
              })}
            />
          ) : (
            <Text
              style={styles.hint}
              accessibilityLabel={t('dialogs.contact.photoAltNone', {
                name: original?.display_name ?? displayName,
              })}
            >
              {t('dialogs.contact.photoNone')}
            </Text>
          )}
          {!viewOnly && (
            <View style={styles.photoActions}>
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t(
                  photo != null
                    ? 'dialogs.contact.photoReplace'
                    : 'dialogs.contact.photoChoose',
                )}
                disabled={busy}
                onPress={() => void pickPhoto()}
                style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
              >
                <Text style={styles.addButtonText}>
                  {t(
                    photo != null
                      ? 'dialogs.contact.photoReplace'
                      : 'dialogs.contact.photoChoose',
                  )}
                </Text>
              </Pressable>
              {photo != null && (
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: busy }}
                  accessibilityLabel={t('dialogs.contact.photoRemove')}
                  disabled={busy}
                  onPress={() => void removePhoto()}
                  style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
                >
                  <Text style={styles.removeButtonText}>
                    {t('dialogs.contact.photoRemove')}
                  </Text>
                </Pressable>
              )}
            </View>
          )}
        </View>
      )}

      {/* Distribution-list switch: turns the person fields into a members
          editor (the wire marks a group by members != null). One switch node
          for SR (the Pressable owns role/checked/label/tap; the inner Switch is
          the visual indicator, hidden + non-interactive). */}
      <Pressable
        accessibilityRole="switch"
        accessibilityState={{ checked: isGroup, disabled: viewOnly }}
        accessibilityLabel={t('dialogs.contact.isGroupLabel')}
        disabled={viewOnly}
        onPress={() => setIsGroup((v) => !v)}
        style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
      >
        <Text style={styles.switchLabel} importantForAccessibility="no">
          {t('dialogs.contact.isGroupLabel')}
        </Text>
        <View pointerEvents="none">
          <Switch
            value={isGroup}
            trackColor={{ false: colors.border, true: colors.accent }}
            importantForAccessibility="no"
            accessibilityElementsHidden
          />
        </View>
      </Pressable>

      {isGroup && (
        <Field
          label={t('dialogs.contact.membersLabel')}
          hint={t('dialogs.contact.membersHint')}
        >
          <TextInput
            style={[styles.input, styles.multiline]}
            value={membersText}
            onChangeText={setMembersText}
            placeholder={t('dialogs.contact.membersPlaceholder')}
            accessibilityLabel={t('dialogs.contact.membersLabel')}
            autoCapitalize="none"
            autoCorrect={false}
            multiline
          />
        </Field>
      )}

      {!isGroup && (
        <>
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

      {!isGroup && (
        <>
          <Field label={t('dialogs.contact.jobTitleLabel')}>
            <TextInput
              style={styles.input}
              value={jobTitle}
              onChangeText={setJobTitle}
              accessibilityLabel={t('dialogs.contact.jobTitleLabel')}
            />
          </Field>

          <Field label={t('dialogs.contact.departmentLabel')}>
            <TextInput
              style={styles.input}
              value={department}
              onChangeText={setDepartment}
              accessibilityLabel={t('dialogs.contact.departmentLabel')}
            />
          </Field>
        </>
      )}

      <ChannelSection
        kind="email"
        values={emails}
        onChange={setEmails}
      />

      {!isGroup && (
        <>
          <ChannelSection
            kind="phone"
            values={phones}
            onChange={setPhones}
          />

          <ChannelSection
            kind="url"
            values={urls}
            onChange={setUrls}
          />
        </>
      )}

      <Field
        label={t('dialogs.contact.birthdayLabel')}
        hint={t('dialogs.contact.birthdayHint')}
      >
        {birthday.trim() === '' ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.contact.birthdayAdd')}
            onPress={() => setBirthday(formatLocalDate(new Date()))}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>{t('dialogs.contact.birthdayAdd')}</Text>
          </Pressable>
        ) : (
          <View style={styles.pickerRow}>
            {/* Accessible field button (value in the label, picker in a
                dialog) — the inline compact picker never joined the VoiceOver
                swipe order. */}
            <DateTimeFieldButton
              label={t('dialogs.contact.birthdayLabel')}
              mode="date"
              value={birthday}
              onChange={setBirthday}
            />
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={t('dialogs.contact.birthdayClear')}
              onPress={() => setBirthday('')}
              style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
            >
              <Text style={styles.ghostButtonText}>
                {t('dialogs.contact.birthdayClear')}
              </Text>
            </Pressable>
          </View>
        )}
      </Field>

      {!isGroup && (
        <Field label={t('dialogs.contact.anniversaryLabel')}>
          {anniversary.trim() === '' ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={t('dialogs.contact.anniversaryAdd')}
              onPress={() => setAnniversary(formatLocalDate(new Date()))}
              style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
            >
              <Text style={styles.ghostButtonText}>
                {t('dialogs.contact.anniversaryAdd')}
              </Text>
            </Pressable>
          ) : (
            <View style={styles.pickerRow}>
              <DateTimeFieldButton
                label={t('dialogs.contact.anniversaryLabel')}
                mode="date"
                value={anniversary}
                onChange={setAnniversary}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.contact.anniversaryClear')}
                onPress={() => setAnniversary('')}
                style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
              >
                <Text style={styles.ghostButtonText}>
                  {t('dialogs.contact.anniversaryClear')}
                </Text>
              </Pressable>
            </View>
          )}
        </Field>
      )}

      {/* Postal addresses — a dynamic list of structured rows. */}
      <View style={styles.field}>
        <Text style={styles.label} accessibilityRole="header">
          {t('dialogs.contact.addressesLabel')}
        </Text>
        {addresses.length === 0 ? (
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.contact.addressesEmpty')}
          </Text>
        ) : (
          addresses.map((addr, i) => (
            <View key={i} style={styles.addressRow}>
              <Text style={styles.addressHeading} accessibilityRole="header">
                {`${t('dialogs.contact.addressesLabel')} ${i + 1}`}
              </Text>
              <Text style={styles.subLabel}>
                {t('dialogs.contact.addressLabel')}
              </Text>
              <TextInput
                ref={addressFocus.registerRow(i)}
                style={styles.input}
                value={addr.label}
                onChangeText={(v) => updateAddress(i, 'label', v)}
                placeholder={t('dialogs.contact.addressLabelHome')}
                accessibilityLabel={`${t('dialogs.contact.addressLabel')}, ${t('dialogs.contact.addressesLabel')} ${i + 1}`}
                autoCapitalize="none"
                autoCorrect={false}
              />
              <Text style={styles.subLabel}>
                {t('dialogs.contact.addressStreet')}
              </Text>
              <TextInput
                style={[styles.input, styles.multiline]}
                value={addr.street}
                onChangeText={(v) => updateAddress(i, 'street', v)}
                accessibilityLabel={t('dialogs.contact.addressStreet')}
                multiline
              />
              <Text style={styles.subLabel}>
                {t('dialogs.contact.addressCity')}
              </Text>
              <TextInput
                style={styles.input}
                value={addr.city}
                onChangeText={(v) => updateAddress(i, 'city', v)}
                accessibilityLabel={t('dialogs.contact.addressCity')}
              />
              <Text style={styles.subLabel}>
                {t('dialogs.contact.addressRegion')}
              </Text>
              <TextInput
                style={styles.input}
                value={addr.region}
                onChangeText={(v) => updateAddress(i, 'region', v)}
                accessibilityLabel={t('dialogs.contact.addressRegion')}
              />
              <Text style={styles.subLabel}>
                {t('dialogs.contact.addressPostalCode')}
              </Text>
              <TextInput
                style={styles.input}
                value={addr.postal_code}
                onChangeText={(v) => updateAddress(i, 'postal_code', v)}
                accessibilityLabel={t('dialogs.contact.addressPostalCode')}
                autoCapitalize="characters"
                autoCorrect={false}
              />
              <Text style={styles.subLabel}>
                {t('dialogs.contact.addressCountry')}
              </Text>
              <TextInput
                style={styles.input}
                value={addr.country}
                onChangeText={(v) => updateAddress(i, 'country', v)}
                accessibilityLabel={t('dialogs.contact.addressCountry')}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t('dialogs.contact.addressRemoveAria', {
                  index: i + 1,
                })}
                onPress={() => removeAddress(i)}
                style={({ pressed }) => [
                  styles.removeButton,
                  pressed && styles.pressed,
                ]}
              >
                <Text style={styles.removeButtonText}>
                  {t('dialogs.contact.addressRemove')}
                </Text>
              </Pressable>
            </View>
          ))
        )}
        <Pressable
          ref={addressFocus.registerAdd}
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.contact.addressAdd')}
          onPress={addAddress}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>
            {t('dialogs.contact.addressAdd')}
          </Text>
        </Pressable>
      </View>
        </>
      )}

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
          disabled={busy || viewOnly}
        />
      )}

      {!viewOnly && (
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
      )}
    </FormScrollView>
  );
}

/** The three labelled channel kinds and the i18n keys / keyboard hints that
 *  differ between them. Everything else about a row is identical, so one
 *  component serves all three instead of three near-copies. */
const CHANNEL_KINDS = {
  email: {
    legend: 'dialogs.contact.emailsLabel',
    empty: 'dialogs.contact.emailsEmpty',
    add: 'dialogs.contact.emailAdd',
    value: 'dialogs.contact.emailValue',
    remove: 'dialogs.contact.emailRemove',
    removeAria: 'dialogs.contact.emailRemoveAria',
    keyboard: 'email-address',
    defaultLabel: 'home',
  },
  phone: {
    legend: 'dialogs.contact.phoneNumbersLabel',
    empty: 'dialogs.contact.phonesEmpty',
    add: 'dialogs.contact.phoneAdd',
    value: 'dialogs.contact.phoneValue',
    remove: 'dialogs.contact.phoneRemove',
    removeAria: 'dialogs.contact.phoneRemoveAria',
    keyboard: 'phone-pad',
    defaultLabel: 'mobile',
  },
  url: {
    legend: 'dialogs.contact.urlsLabel',
    empty: 'dialogs.contact.urlsEmpty',
    add: 'dialogs.contact.urlAdd',
    value: 'dialogs.contact.urlValue',
    remove: 'dialogs.contact.urlRemove',
    removeAria: 'dialogs.contact.urlRemoveAria',
    keyboard: 'url',
    defaultLabel: 'home',
  },
} as const;

type ChannelKind = keyof typeof CHANNEL_KINDS;

/** One list of labelled values with an Add button below — the same
 *  repeating-row shape as the postal addresses further down the form, so the
 *  editor has one idiom rather than two.
 *
 *  Each row is three stops: the label button, the value, Remove. The label
 *  lives on a button rather than an inline radio group precisely so a contact
 *  with four numbers doesn't cost two dozen swipes to walk past. */
function ChannelSection({
  kind,
  values,
  onChange,
}: {
  kind: ChannelKind;
  values: ContactValue[];
  onChange: (next: ContactValue[]) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const spec = CHANNEL_KINDS[kind];
  const focus = useListFocusManager(values.length);
  return (
    <View style={styles.field}>
      <Text style={styles.label} accessibilityRole="header">
        {t(spec.legend)}
      </Text>
      {values.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t(spec.empty)}
        </Text>
      ) : (
        values.map((entry, i) => {
          const rowName = `${t(spec.value)} ${i + 1}`;
          return (
            <View key={i} style={styles.addressRow}>
              <ContactLabelButton
                label={entry.label}
                fieldLabel={rowName}
                onChange={(label) =>
                  onChange(values.map((v, j) => (j === i ? { ...v, label } : v)))
                }
              />
              <TextInput
                ref={focus.registerRow(i)}
                style={styles.input}
                value={entry.value}
                onChangeText={(value) =>
                  onChange(values.map((v, j) => (j === i ? { ...v, value } : v)))
                }
                accessibilityLabel={rowName}
                autoCapitalize="none"
                autoCorrect={false}
                keyboardType={spec.keyboard}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={t(spec.removeAria, { index: i + 1 })}
                onPress={() => {
                  focus.onRemove(i);
                  onChange(values.filter((_, j) => j !== i));
                }}
                style={({ pressed }) => [
                  styles.removeButton,
                  pressed && styles.pressed,
                ]}
              >
                <Text style={styles.removeButtonText}>{t(spec.remove)}</Text>
              </Pressable>
            </View>
          );
        })
      )}
      <Pressable
        ref={focus.registerAdd}
        accessibilityRole="button"
        accessibilityLabel={t(spec.add)}
        onPress={() => {
          // `onAdd` is what arms the hook's pending target; without it the
          // count-keyed effect is a no-op, the cursor stays on the button and
          // nothing is spoken — while "Add address" right below this list does
          // move into its new row. Two lists in one screen behaving
          // differently is the bug.
          focus.onAdd();
          onChange([...values, { value: '', label: spec.defaultLabel }]);
        }}
        style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
      >
        <Text style={styles.addButtonText}>{t(spec.add)}</Text>
      </Pressable>
    </View>
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
  const styles = useThemedStyles(makeStyles);
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

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    field: { gap: 6 },
    pickerRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      flexWrap: 'wrap',
    },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    multiline: { minHeight: 88, textAlignVertical: 'top' },
    photo: {
      width: 96,
      height: 96,
      borderRadius: 48,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    photoActions: { flexDirection: 'row', gap: 10, flexWrap: 'wrap' },
    switchRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    switchLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
    addressRow: {
      gap: 6,
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    addressHeading: { fontSize: 15, fontWeight: '700', color: c.textPrimary },
    subLabel: { fontSize: 13, fontWeight: '600', color: c.textSecondary },
    removeButton: {
      marginTop: 4,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
      alignSelf: 'flex-start',
    },
    removeButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    addButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    primaryButton: {
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    readOnlyBanner: {
      fontSize: 14,
      color: c.textSecondary,
      fontWeight: '600',
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceSubtle,
    },
  });
