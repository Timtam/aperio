import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../../a11y/Announcer';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import type { Contact, ContactList } from '../../api/types';
import { useCalendarStore } from '../../state/CalendarStore';
import { useContacts } from '../../state/useContacts';
import { useDialogState } from '../../state/DialogState';

/**
 * Contacts view (DESIGN.md §10).
 *
 * Listbox of contacts grouped by address book — same ARIA shape
 * as `TaskView`, just without the recurrence / status flourishes.
 *
 * Keyboard:
 *   - Arrow Up/Down move between contacts (separators are skipped)
 *   - Home / End jump to first / last contact
 *   - Enter opens the dialog in edit mode
 *   - Insert / Ctrl+N opens the dialog in create mode
 *
 * Search and the attendees autocomplete (§10.4) come in later
 * phases; this view is the catalog screen that lets the user pile
 * contacts in / out manually first.
 */
export function ContactsView() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { contactLists } = useCalendarStore();
  const { contacts, loading, contactListById } = useContacts();
  const { openContactDialog } = useDialogState();

  const headingId = useId();
  const sectionRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const [focusIndex, setFocusIndex] = useState(0);
  const [listHasFocus, setListHasFocus] = useState(false);

  const showLoading = useDeferredLoading(loading);

  // Flat list of {separator | contact} entries — separators carry
  // a contact-list header, contacts hold the row data. focusIndex
  // points at the *contact* index in `flatContacts`; separators
  // never receive focus.
  const { entries, flatContacts } = useMemo(() => {
    return buildEntries(contacts, contactListById, contactLists, t);
  }, [contacts, contactListById, contactLists, t]);

  useEffect(() => {
    if (focusIndex >= flatContacts.length) {
      setFocusIndex(Math.max(0, flatContacts.length - 1));
    }
  }, [flatContacts.length, focusIndex]);

  const focusedContact: Contact | null = flatContacts[focusIndex] ?? null;

  const openEdit = useCallback(
    (c: Contact) => {
      openContactDialog(c);
    },
    [openContactDialog],
  );

  const openCreate = useCallback(() => {
    // Prefer the focused contact's list (so the user can iterate
    // through one book), fall back to whatever the dialog's own
    // resolution chain picks.
    openContactDialog(null, { listId: focusedContact?.list_id });
  }, [focusedContact, openContactDialog]);

  const handleListKey = (e: ReactKeyboardEvent<HTMLUListElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (flatContacts.length === 0) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setFocusIndex((i) => Math.min(i + 1, flatContacts.length - 1));
        return;
      case 'ArrowUp':
        e.preventDefault();
        setFocusIndex((i) => Math.max(i - 1, 0));
        return;
      case 'Home':
        e.preventDefault();
        setFocusIndex(0);
        return;
      case 'End':
        e.preventDefault();
        setFocusIndex(flatContacts.length - 1);
        return;
      case 'Enter':
        e.preventDefault();
        if (focusedContact) openEdit(focusedContact);
        return;
      case 'Insert':
        e.preventDefault();
        openCreate();
        return;
      default:
        return;
    }
  };

  // SR live-region updates when the focused row changes — same
  // pattern TaskView uses to surface "Max Mustermann, Inbox,
  // Eintrag 3 von 14".
  useEffect(() => {
    if (!listHasFocus) return;
    if (!focusedContact) return;
    const list = contactListById.get(focusedContact.list_id);
    announce(
      t('views.contacts.focusAnnounce', {
        name: focusedContact.display_name,
        list: list?.name ?? '',
        index: focusIndex + 1,
        total: flatContacts.length,
      }),
    );
    // Only the focused row's identity matters — re-announce on
    // every focus move, but not on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusIndex, listHasFocus]);

  const optionId = (i: number) => `${headingId}-row-${i}`;

  const sectionTabIndex = flatContacts.length === 0 ? 0 : -1;

  return (
    <section
      ref={sectionRef}
      tabIndex={sectionTabIndex}
      aria-labelledby={`${headingId}-heading`}
      aria-describedby={
        flatContacts.length === 0 ? `${headingId}-empty` : undefined
      }
      className="contacts-view"
    >
      <header className="contacts-view__header">
        <h2 id={`${headingId}-heading`} className="contacts-view__title">
          {t('views.contacts.heading')}
        </h2>
        <div className="contacts-view__actions">
          <button
            type="button"
            className="form__action form__action--primary"
            onClick={openCreate}
          >
            {t('views.contacts.add')}
          </button>
        </div>
      </header>

      {showLoading && (
        <p role="status" className="form__hint">
          {t('views.contacts.loading')}
        </p>
      )}

      {!showLoading && flatContacts.length === 0 && (
        <p id={`${headingId}-empty`} className="form__hint">
          {t('views.contacts.empty')}
        </p>
      )}

      {flatContacts.length > 0 && (
        <ul
          ref={listRef}
          role="listbox"
          tabIndex={0}
          aria-label={t('views.contacts.listLabel')}
          aria-activedescendant={
            listHasFocus ? optionId(focusIndex) : undefined
          }
          onFocus={() => setListHasFocus(true)}
          onBlur={() => setListHasFocus(false)}
          onKeyDown={handleListKey}
          className="contacts-list"
        >
          {entries.map((entry) => {
            if (entry.kind === 'separator') {
              return (
                <li
                  key={entry.key}
                  role="presentation"
                  className="contacts-list__separator"
                >
                  <span>{entry.label}</span>
                </li>
              );
            }
            const focused = entry.index === focusIndex;
            const list = contactListById.get(entry.contact.list_id);
            const accessibleName = buildAriaLabel(entry.contact, list, t);
            return (
              <li
                key={entry.contact.id}
                id={optionId(entry.index)}
                role="option"
                aria-selected={listHasFocus ? focused : undefined}
                aria-label={accessibleName}
                className={
                  'contacts-list__item' +
                  (focused ? ' contacts-list__item--focused' : '')
                }
                onClick={() => {
                  setFocusIndex(entry.index);
                  openEdit(entry.contact);
                }}
              >
                <span className="contacts-list__name">
                  {entry.contact.display_name}
                </span>
                {(entry.contact.organization ||
                  entry.contact.emails.length > 0) && (
                  <span className="contacts-list__meta">
                    {entry.contact.organization && (
                      <span className="contacts-list__org">
                        {entry.contact.organization}
                      </span>
                    )}
                    {entry.contact.emails[0] && (
                      <span className="contacts-list__email">
                        {entry.contact.emails[0]}
                      </span>
                    )}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

// ─────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────

type Entry =
  | { kind: 'separator'; key: string; label: string }
  | { kind: 'contact'; key: string; index: number; contact: Contact };

function buildEntries(
  contacts: Contact[],
  contactListById: Map<string, ContactList>,
  contactLists: ContactList[],
  t: (key: string, values?: Record<string, unknown>) => string,
): { entries: Entry[]; flatContacts: Contact[] } {
  // Group contacts by list_id. The hook already sorts the global
  // list by display_name, so within each bucket we keep that
  // order. Lists themselves are emitted in the order they appear
  // in `contactLists` (which the calendar store sorts via the
  // backend — local first, then alphabetical).
  const byList = new Map<string, Contact[]>();
  for (const c of contacts) {
    const arr = byList.get(c.list_id) ?? [];
    arr.push(c);
    byList.set(c.list_id, arr);
  }

  const entries: Entry[] = [];
  const flatContacts: Contact[] = [];

  for (const list of contactLists) {
    const bucket = byList.get(list.id) ?? [];
    if (bucket.length === 0) continue;
    entries.push({
      kind: 'separator',
      key: `sep:${list.id}`,
      label: t('views.contacts.groupLabel', {
        name: list.name,
        count: bucket.length,
      }),
    });
    for (const c of bucket) {
      entries.push({
        kind: 'contact',
        key: `row:${c.id}`,
        index: flatContacts.length,
        contact: c,
      });
      flatContacts.push(c);
    }
  }

  // Catch contacts whose owning list isn't (yet) in the catalog —
  // a transient state during a refresh, or a row that got orphaned
  // by a failed cascade. Better to show them under a generic
  // bucket than to silently drop them.
  const seen = new Set(contactLists.map((l) => l.id));
  const orphans = contacts.filter((c) => !seen.has(c.list_id));
  if (orphans.length > 0) {
    entries.push({
      kind: 'separator',
      key: 'sep:_orphans',
      label: t('views.contacts.groupOrphans', { count: orphans.length }),
    });
    for (const c of orphans) {
      entries.push({
        kind: 'contact',
        key: `row:${c.id}`,
        index: flatContacts.length,
        contact: c,
      });
      flatContacts.push(c);
    }
  }
  // Silence unused-warning when `contactListById` is only used by
  // the rendering side. (We pass it in case future entries want a
  // direct lookup.)
  void contactListById;

  return { entries, flatContacts };
}

function buildAriaLabel(
  c: Contact,
  list: ContactList | undefined,
  t: (key: string, values?: Record<string, unknown>) => string,
): string {
  // Stack: name → org → primary email → list. Each component is
  // skipped when empty so the screen reader doesn't read empty
  // commas.
  const parts: string[] = [c.display_name];
  if (c.organization) parts.push(c.organization);
  if (c.emails[0]) parts.push(c.emails[0]);
  if (list) parts.push(t('views.contacts.fromList', { list: list.name }));
  return parts.join(', ');
}
