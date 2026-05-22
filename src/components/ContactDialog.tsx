import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createContact as apiCreateContact,
  deleteContact as apiDeleteContact,
  isCommandError,
  updateContact as apiUpdateContact,
} from '../api/client';
import type { Contact } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { useDialogState } from '../state/DialogState';
import { ConfirmDialog } from './ConfirmDialog';
import { Modal } from './Modal';

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
  };
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
    queueMicrotask(() => firstFieldRef.current?.focus());
  }, [isOpen, contact, resolveDefaultListId]);

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

      const payload = {
        display_name: name,
        given_name: form.givenName.trim() || null,
        family_name: form.familyName.trim() || null,
        organization: form.organization.trim() || null,
        emails: splitCsv(form.emails),
        phone_numbers: splitCsv(form.phoneNumbers),
        birthday: form.birthday || null,
        notes: form.notes.trim() || null,
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
          announce(
            t('dialogs.contact.updated', { name: payload.display_name }),
          );
        } else {
          await apiCreateContact({
            list_id: form.listId,
            ...payload,
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
    [form, contact, t, announce, invalidateData, onClose],
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
