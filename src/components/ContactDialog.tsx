import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createContact as apiCreateContact,
  deleteContact as apiDeleteContact,
  deleteContactPhoto as apiDeleteContactPhoto,
  getContactPhoto as apiGetContactPhoto,
  isCommandError,
  setContactPhoto as apiSetContactPhoto,
  updateContact as apiUpdateContact,
} from '../api/client';
import type { Contact, ContactPhoto } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { useDialogState } from '../state/DialogState';
import { ConfirmDialog } from './ConfirmDialog';
import { Modal } from './Modal';

/** File-size cap for an uploaded avatar. Five megabytes is what
 *  every CardDAV / Exchange server we target accepts comfortably
 *  — pushing past it tends to surface as `ErrorRequestStreamTooLarge`
 *  on EWS or a 413 from Nextcloud. We enforce in the dialog so the
 *  failure mode is a clear inline error rather than a backend
 *  round-trip that comes back red. */
const MAX_PHOTO_BYTES = 5 * 1024 * 1024;

/** MIME types the upload widget accepts. Matches what every
 *  CardDAV and EWS implementation reliably round-trips; HEIC/HEIF
 *  (Apple's default) we deliberately exclude — Outlook can't
 *  render it, and re-encoding browser-side would mean shipping a
 *  decoder. */
const ALLOWED_PHOTO_TYPES = ['image/jpeg', 'image/png', 'image/gif'];

/**
 * Contact create / edit dialog (DESIGN.md §10).
 *
 * Phase 10a-3 scope: the core data-model fields. Emails and phone
 * numbers are multi-valued in the model but the dialog edits them
 * as comma-separated strings — a dedicated multi-row editor lands
 * with the attendees picker in 10c when that UX needs it.
 *
 * Save dispatches `create_contact` (new) or `update_contact`
 * (edit); a `Delete` button in the footer drops the row outright
 * with a single confirmation step. All paths bump `dataVersion`
 * via the dialog provider's auto-invalidate, so the list view
 * re-renders without each caller having to remember.
 */
export interface ContactDialogProps {
  isOpen: boolean;
  onClose: () => void;
  contact: Contact | null;
  /** Pre-select this contact list when creating a new contact. */
  defaultListId?: string;
}

interface FormState {
  listId: string;
  displayName: string;
  givenName: string;
  familyName: string;
  organization: string;
  emails: string;
  phoneNumbers: string;
  birthday: string;
  notes: string;
  /** Toggles distribution-list mode. When true, the dialog hides
   *  person-only fields (given/family/birthday/phone) and surfaces
   *  the member textarea instead. Stored separately from
   *  `membersText` so toggling off and on doesn't wipe whatever
   *  the user already typed. */
  isGroup: boolean;
  /** One member per line, `Name <email@example.com>` style — same
   *  RFC 2822 shape mail clients have trained users on. Bare
   *  email addresses are also accepted; the name defaults to null
   *  on parse. */
  membersText: string;
}

function emptyForm(): FormState {
  return {
    listId: '',
    displayName: '',
    givenName: '',
    familyName: '',
    organization: '',
    emails: '',
    phoneNumbers: '',
    birthday: '',
    notes: '',
    isGroup: false,
    membersText: '',
  };
}

function fromContact(c: Contact): FormState {
  return {
    listId: c.list_id,
    displayName: c.display_name,
    givenName: c.given_name ?? '',
    familyName: c.family_name ?? '',
    organization: c.organization ?? '',
    emails: c.emails.join(', '),
    phoneNumbers: c.phone_numbers.join(', '),
    birthday: c.birthday ?? '',
    notes: c.notes ?? '',
    isGroup: c.members !== null,
    membersText: (c.members ?? [])
      .map((m) =>
        m.name && m.name.trim()
          ? `${m.name.trim()} <${m.email}>`
          : m.email,
      )
      .join('\n'),
  };
}

/** Parse the multi-line members textarea into structured
 *  GroupMember records. Each non-empty line is either
 *  `Name <email>` (CN preserved) or a bare `email` (name = null).
 *  Lines without an `@` are skipped — the picker needs an email
 *  to be useful. */
function parseMembers(raw: string): { name: string | null; email: string }[] {
  const out: { name: string | null; email: string }[] = [];
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const angle = trimmed.match(/^(.*?)\s*<\s*([^>]+?)\s*>\s*$/);
    if (angle) {
      const name = angle[1].trim();
      const email = angle[2].trim();
      if (email.includes('@')) {
        out.push({ name: name || null, email });
      }
      continue;
    }
    if (trimmed.includes('@')) {
      out.push({ name: null, email: trimmed });
    }
  }
  return out;
}

/** Split a comma-separated string into trimmed non-empty entries.
 *  The DB stores `emails` / `phone_numbers` as JSON arrays of
 *  strings; we round-trip via this helper so editing a contact
 *  with `["a@x.com", "b@x.com"]` displays as `a@x.com, b@x.com`
 *  and back. */
function splitCsv(raw: string): string[] {
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** Read a `File` into the base64 `ContactPhoto` shape the
 *  backend expects. We strip the data URI prefix so what travels
 *  is just the base64 body — the Rust side's custom serde
 *  deserialises that into `Vec<u8>`. */
function fileToContactPhoto(file: File): Promise<ContactPhoto> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('read failed'));
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== 'string') {
        reject(new Error('FileReader returned non-string result'));
        return;
      }
      const comma = result.indexOf(',');
      const b64 = comma >= 0 ? result.slice(comma + 1) : result;
      resolve({ content_type: file.type, data: b64 });
    };
    reader.readAsDataURL(file);
  });
}

/** Render a `ContactPhoto` as a `data:` URL the browser can paint
 *  into an `<img>`. Falls back to an empty string for nullish
 *  inputs so callers can pass it through unconditionally. */
function photoToDataUrl(photo: ContactPhoto | null): string {
  if (!photo) return '';
  return `data:${photo.content_type};base64,${photo.data}`;
}

export function ContactDialog({
  isOpen,
  onClose,
  contact,
  defaultListId,
}: ContactDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { contactLists } = useCalendarStore();
  const { invalidateData } = useDialogState();

  const isEdit = contact !== null;
  const titleId = useId();
  const firstFieldRef = useRef<HTMLInputElement>(null);

  // Editable contact lists only — read-only books appear in the
  // dropdown grayed out so the user can see them, but new contacts
  // can't land there. Edited contacts stay in their list even if
  // the list is read-only (we don't surface the dropdown for those
  // on the create side, but the existing list_id is preserved).
  const writableLists = useMemo(
    () => contactLists.filter((l) => !l.read_only),
    [contactLists],
  );

  // Default list resolution chain:
  //   1. Editing → the contact's own list_id (locked, see below).
  //   2. Caller-provided `defaultListId` if it's writable.
  //   3. First writable list (typically `local-default-contacts`).
  //   4. Fallback: first list overall (would be a misconfiguration —
  //      no writable lists at all — but better than blank state).
  const resolveDefaultListId = useCallback((): string => {
    if (contact) return contact.list_id;
    if (defaultListId && writableLists.some((l) => l.id === defaultListId)) {
      return defaultListId;
    }
    return writableLists[0]?.id ?? contactLists[0]?.id ?? '';
  }, [contact, defaultListId, writableLists, contactLists]);

  const [form, setForm] = useState<FormState>(emptyForm);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  // Photo state. `photo` is the bytes currently held in the
  // dialog (either freshly chosen by the user or fetched from
  // the server on open); `photoLoading` is the in-flight fetch
  // indicator. `photoDirty` flips when the user picks a new
  // photo OR removes the existing one, telling the save handler
  // to issue the set / delete round-trip in addition to the
  // contact update.
  const [photo, setPhoto] = useState<ContactPhoto | null>(null);
  const [photoLoading, setPhotoLoading] = useState(false);
  const [photoDirty, setPhotoDirty] = useState(false);
  const [photoError, setPhotoError] = useState<string | null>(null);
  const photoInputRef = useRef<HTMLInputElement>(null);
  const photoInputId = useId();

  // Reset / hydrate the form whenever the dialog opens or the
  // editing target swaps. Putting focus on the display-name field
  // is the standard treatment — that's almost always the first
  // thing the user wants to type.
  useEffect(() => {
    if (!isOpen) return;
    if (contact) {
      setForm(fromContact(contact));
    } else {
      setForm({
        ...emptyForm(),
        listId: resolveDefaultListId(),
      });
    }
    setError(null);
    setConfirmDelete(false);
    // Reset photo state on every open. We re-fetch below when
    // editing a contact that claims to have one.
    setPhoto(null);
    setPhotoDirty(false);
    setPhotoError(null);
    setPhotoLoading(false);
    queueMicrotask(() => firstFieldRef.current?.focus());
  }, [isOpen, contact, resolveDefaultListId]);

  // Lazy photo fetch: only when editing a contact whose listing
  // flag claims it has one. Cancellable so a fast close-then-open
  // doesn't race a stale fetch onto the new dialog.
  useEffect(() => {
    if (!isOpen || !contact || !contact.has_photo) {
      return;
    }
    let cancelled = false;
    setPhotoLoading(true);
    setPhotoError(null);
    apiGetContactPhoto(contact.id, contact.list_id)
      .then((p) => {
        if (cancelled) return;
        setPhoto(p);
        setPhotoLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        setPhotoError(t('dialogs.contact.photoLoadFailed'));
        setPhotoLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, contact, t]);

  const onPickPhoto = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      // Reset the input value so picking the same file twice in a
      // row still fires onChange (a no-op pick wouldn't otherwise
      // surface, leaving the user wondering why the preview
      // didn't update).
      event.target.value = '';
      if (!file) return;
      if (!ALLOWED_PHOTO_TYPES.includes(file.type)) {
        setPhotoError(t('dialogs.contact.photoUnsupportedType'));
        return;
      }
      if (file.size > MAX_PHOTO_BYTES) {
        setPhotoError(
          t('dialogs.contact.photoTooLarge', {
            limit: `${MAX_PHOTO_BYTES / (1024 * 1024)} MB`,
          }),
        );
        return;
      }
      try {
        const next = await fileToContactPhoto(file);
        setPhoto(next);
        setPhotoDirty(true);
        setPhotoError(null);
      } catch (err) {
        setPhotoError(String(err));
      }
    },
    [t],
  );

  const onRemovePhoto = useCallback(() => {
    setPhoto(null);
    setPhotoDirty(true);
    setPhotoError(null);
  }, []);

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      const name = form.displayName.trim();
      if (!name) {
        setError(t('dialogs.contact.displayNameRequired'));
        return;
      }
      if (!form.listId) {
        setError(t('dialogs.contact.listRequired'));
        return;
      }
      setSubmitting(true);
      setError(null);

      // Group payload differs from a person payload: when the user
      // marked this contact as a distribution list, we wipe the
      // person-only fields (given/family/birthday/phone) and emit
      // the parsed members array. The backend uses the non-null
      // `members` marker to route the row into the right wire
      // shape (`<t:DistributionList>` on EWS, `KIND:group` on
      // CardDAV, the `members` JSON column on local SQLite).
      const payload = form.isGroup
        ? {
            display_name: name,
            given_name: null,
            family_name: null,
            organization: form.organization.trim() || null,
            emails: [],
            phone_numbers: [],
            birthday: null,
            notes: form.notes.trim() || null,
            members: parseMembers(form.membersText),
          }
        : {
            display_name: name,
            given_name: form.givenName.trim() || null,
            family_name: form.familyName.trim() || null,
            organization: form.organization.trim() || null,
            emails: splitCsv(form.emails),
            phone_numbers: splitCsv(form.phoneNumbers),
            birthday: form.birthday || null,
            notes: form.notes.trim() || null,
            members: null,
          };

      try {
        if (contact) {
          await apiUpdateContact({
            ...contact,
            ...payload,
            list_id: form.listId,
            // Bump updated_at locally so optimistic UIs (none yet,
            // but the cache pattern across the app would notice)
            // can compare timestamps; the backend overwrites this
            // with its own clock anyway.
            updated_at: new Date().toISOString(),
          });
          // Photo: only round-trip the change when it actually
          // moved. The non-photo `update_contact` path explicitly
          // does NOT touch photo storage, so untouched photos
          // survive the field update without us doing anything.
          if (photoDirty) {
            if (photo) {
              await apiSetContactPhoto(contact.id, photo, form.listId);
              announce(
                t('dialogs.contact.photoUpdated', {
                  name: payload.display_name,
                }),
              );
            } else {
              await apiDeleteContactPhoto(contact.id, form.listId);
              announce(
                t('dialogs.contact.photoRemoved', {
                  name: payload.display_name,
                }),
              );
            }
          }
          announce(
            t('dialogs.contact.updated', { name: payload.display_name }),
          );
        } else {
          // Create path: the inline `photo` field on NewContact
          // lets the adapter write the avatar in the same logical
          // operation (one local INSERT, one EWS CreateItem +
          // CreateAttachment, one CardDAV PUT with PHOTO embedded).
          await apiCreateContact({
            list_id: form.listId,
            ...payload,
            photo: photo,
          });
          announce(
            t('dialogs.contact.created', { name: payload.display_name }),
          );
        }
        invalidateData();
        onClose();
      } catch (err) {
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      } finally {
        setSubmitting(false);
      }
    },
    [
      form,
      contact,
      photo,
      photoDirty,
      t,
      announce,
      invalidateData,
      onClose,
    ],
  );

  const performDelete = useCallback(async () => {
    if (!contact) return;
    setSubmitting(true);
    setError(null);
    try {
      await apiDeleteContact(contact.id, contact.list_id);
      announce(
        t('dialogs.contact.deleted', { name: contact.display_name }),
      );
      invalidateData();
      onClose();
    } catch (err) {
      if (isCommandError(err)) {
        setError(`${err.code}: ${err.message}`);
      } else {
        setError(String(err));
      }
    } finally {
      setSubmitting(false);
    }
  }, [contact, t, announce, invalidateData, onClose]);

  return (
    <>
      <Modal
        isOpen={isOpen}
        onClose={onClose}
        title={t(
          isEdit ? 'dialogs.contact.editTitle' : 'dialogs.contact.createTitle',
        )}
        className="modal--form modal--contact"
        dismissOnBackdrop={false}
      >
        <form className="form" onSubmit={onSubmit}>
          {error && (
            <p role="alert" className="form__error">
              {error}
            </p>
          )}

          <div className="form__field form__field--photo">
            <span className="form__label">
              {t('dialogs.contact.photoSectionLabel')}
            </span>
            <div className="contact-photo">
              {photo ? (
                <img
                  className="contact-photo__preview"
                  src={photoToDataUrl(photo)}
                  alt={t('dialogs.contact.photoAltSet', {
                    name: form.displayName || t('dialogs.contact.photoNone'),
                  })}
                />
              ) : (
                <div
                  className="contact-photo__placeholder"
                  role="img"
                  aria-label={t('dialogs.contact.photoAltNone', {
                    name: form.displayName || '',
                  })}
                >
                  {photoLoading
                    ? t('dialogs.contact.photoLoading')
                    : t('dialogs.contact.photoNone')}
                </div>
              )}
              <div className="contact-photo__actions">
                <input
                  ref={photoInputRef}
                  id={photoInputId}
                  type="file"
                  accept={ALLOWED_PHOTO_TYPES.join(',')}
                  className="sr-only"
                  onChange={(e) => void onPickPhoto(e)}
                />
                <button
                  type="button"
                  className="form__action"
                  onClick={() => photoInputRef.current?.click()}
                  aria-disabled={submitting || photoLoading || undefined}
                >
                  {photo
                    ? t('dialogs.contact.photoReplace')
                    : t('dialogs.contact.photoChoose')}
                </button>
                {photo && (
                  <button
                    type="button"
                    className="form__action"
                    onClick={onRemovePhoto}
                    aria-disabled={submitting || undefined}
                  >
                    {t('dialogs.contact.photoRemove')}
                  </button>
                )}
              </div>
              {photoError && (
                <p role="alert" className="form__error">
                  {photoError}
                </p>
              )}
            </div>
          </div>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.displayNameLabel')}
            </span>
            <input
              ref={firstFieldRef}
              type="text"
              value={form.displayName}
              onChange={(e) =>
                setForm((p) => ({ ...p, displayName: e.target.value }))
              }
              autoComplete="off"
              spellCheck={false}
              required
            />
          </label>

          <div className="form__row form__row--two">
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.contact.givenNameLabel')}
              </span>
              <input
                type="text"
                value={form.givenName}
                onChange={(e) =>
                  setForm((p) => ({ ...p, givenName: e.target.value }))
                }
                autoComplete="given-name"
              />
            </label>
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.contact.familyNameLabel')}
              </span>
              <input
                type="text"
                value={form.familyName}
                onChange={(e) =>
                  setForm((p) => ({ ...p, familyName: e.target.value }))
                }
                autoComplete="family-name"
              />
            </label>
          </div>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.organizationLabel')}
            </span>
            <input
              type="text"
              value={form.organization}
              onChange={(e) =>
                setForm((p) => ({ ...p, organization: e.target.value }))
              }
              autoComplete="organization"
            />
          </label>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.emailsLabel')}
            </span>
            <input
              type="text"
              value={form.emails}
              onChange={(e) =>
                setForm((p) => ({ ...p, emails: e.target.value }))
              }
              placeholder={t('dialogs.contact.emailsPlaceholder')}
              autoComplete="off"
              spellCheck={false}
            />
            <span className="form__hint">
              {t('dialogs.contact.emailsHint')}
            </span>
          </label>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.phoneNumbersLabel')}
            </span>
            <input
              type="text"
              value={form.phoneNumbers}
              onChange={(e) =>
                setForm((p) => ({ ...p, phoneNumbers: e.target.value }))
              }
              placeholder={t('dialogs.contact.phoneNumbersPlaceholder')}
              autoComplete="off"
              spellCheck={false}
            />
            <span className="form__hint">
              {t('dialogs.contact.phoneNumbersHint')}
            </span>
          </label>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.birthdayLabel')}
            </span>
            <input
              type="date"
              value={form.birthday}
              onChange={(e) =>
                setForm((p) => ({ ...p, birthday: e.target.value }))
              }
            />
          </label>

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.notesLabel')}
            </span>
            <textarea
              value={form.notes}
              onChange={(e) =>
                setForm((p) => ({ ...p, notes: e.target.value }))
              }
              rows={3}
            />
          </label>

          <label className="form__field form__field--check">
            <input
              type="checkbox"
              checked={form.isGroup}
              onChange={(e) =>
                setForm((p) => ({ ...p, isGroup: e.target.checked }))
              }
            />
            <span className="form__label">
              {t('dialogs.contact.isGroupLabel')}
            </span>
          </label>

          {form.isGroup && (
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.contact.membersLabel')}
              </span>
              <textarea
                value={form.membersText}
                onChange={(e) =>
                  setForm((p) => ({ ...p, membersText: e.target.value }))
                }
                placeholder={t('dialogs.contact.membersPlaceholder')}
                rows={5}
                spellCheck={false}
              />
              <span className="form__hint">
                {t('dialogs.contact.membersHint')}
              </span>
            </label>
          )}

          <label className="form__field">
            <span className="form__label">
              {t('dialogs.contact.listLabel')}
            </span>
            <select
              value={form.listId}
              onChange={(e) =>
                setForm((p) => ({ ...p, listId: e.target.value }))
              }
              required
            >
              {contactLists.map((l) => (
                <option key={l.id} value={l.id} disabled={l.read_only}>
                  {l.name}
                  {l.read_only ? ` (${t('dialogs.contact.readOnly')})` : ''}
                </option>
              ))}
            </select>
            <span id={titleId} className="sr-only" />
          </label>

          <div className="form__actions">
            <button
              type="button"
              className="form__action"
              onClick={onClose}
              aria-disabled={submitting || undefined}
            >
              {t('dialogs.contact.cancel')}
            </button>
            {isEdit && (
              <button
                type="button"
                className="form__action form__action--danger"
                onClick={() => setConfirmDelete(true)}
                aria-disabled={submitting || undefined}
              >
                {t('dialogs.contact.delete')}
              </button>
            )}
            <button
              type="submit"
              className="form__action form__action--primary"
              aria-disabled={submitting || undefined}
            >
              {isEdit
                ? t('dialogs.contact.save')
                : t('dialogs.contact.create')}
            </button>
          </div>
        </form>
      </Modal>

      <ConfirmDialog
        isOpen={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        onConfirm={() => void performDelete()}
        title={t('dialogs.contact.deleteTitle')}
        message={t('dialogs.contact.deleteMessage', {
          name: contact?.display_name ?? '',
        })}
      />
    </>
  );
}
