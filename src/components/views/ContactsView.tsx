import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { searchContacts as apiSearchContacts } from '../../api/client';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import type { Contact, ContactList } from '../../api/types';
import { getContactListDisplayName } from '../../intl/contactList';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useContacts } from '../../state/useContacts';
import { useContactSync } from '../../state/useContactSync';
import { useDialogState } from '../../state/dialogStateContext';

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
 * The earlier hard-freeze on big directory listings (~2000 GAL
 * entries) was driven by `aria-setsize` + `content-visibility:
 * auto` on the rendered options — Chromium's accessibility tree
 * and NVDA's virtual cursor combined poorly with those hooks.
 * With both removed, a plain "render every entry" listbox holds
 * up; this file used to ship a sliding window + IntersectionObserver
 * sentinel + progressive reveal hook to limit DOM size, but they
 * each introduced their own re-render storms (reveal-restart
 * loops, observer churn on every reveal tick) and the resulting
 * post-load flicker was worse than the original load cost.
 * The search input above the list is the practical fast path when
 * the user is hunting for a specific person.
 */

export function ContactsView() {
  const { t, i18n } = useTranslation();
  const { contactLists, selectedContactListIds } = useCalendarStore();
  const { contacts, loading, contactListById } = useContacts();
  const { openContactDialog } = useDialogState();
  // Phase 10j: contact sync scheduler bridge. Sets up the
  // `contacts-synced` event listener that re-drives `useContacts`
  // after every periodic / manual / app-start pass, and exposes
  // the trigger function for the Refresh button below.
  const { status: syncStatus, triggering, triggerSync } = useContactSync();

  const headingId = useId();
  const searchId = useId();
  // Auto-focus the listbox once contacts are loaded so the user
  // lands somewhere actionable on mount + on every view-switch
  // (Ctrl+8). Same pattern TaskView uses for its task list.
  // Empty / search-empty states are rendered as a presentation
  // `<li>` *inside* the listbox so the listbox is always the
  // focus target, even before the first contact arrives.
  const listRef = useAutoFocus<HTMLUListElement>(true);
  const [focusIndex, setFocusIndex] = useState(0);
  const [listHasFocus, setListHasFocus] = useState(false);

  // Search input + debounced query the server-side fan-out
  // listens to. We keep two values: `searchInput` is the raw
  // box content (updates on every keystroke for instant UI
  // feedback) and `searchQuery` is the debounced version that
  // actually fires the `search_contacts` Tauri command. 250 ms
  // is the conventional debounce window for typeahead — slow
  // enough to skip the burst of intermediate strings, fast
  // enough to feel responsive.
  const [searchInput, setSearchInput] = useState('');
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Contact[]>([]);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setSearchQuery(searchInput.trim());
    }, 250);
    return () => window.clearTimeout(timer);
  }, [searchInput]);

  useEffect(() => {
    if (!searchQuery) {
      setSearchResults([]);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    apiSearchContacts(searchQuery)
      .then((rows) => {
        if (cancelled) return;
        setSearchResults(rows);
        setSearching(false);
      })
      .catch(() => {
        if (cancelled) return;
        // Per-fetch errors (server hiccup, network) collapse to
        // an empty result rather than bubbling up — the search
        // box stays usable, the next keystroke retries.
        setSearchResults([]);
        setSearching(false);
      });
    return () => {
      cancelled = true;
    };
  }, [searchQuery]);

  // While searching: display the server fan-out results,
  // filtered by which lists the user has selected (so the GAL
  // toggle still controls whether directory hits show up).
  // Otherwise: display the auto-fetched writable lists.
  const isSearching = searchQuery.length > 0;
  const displayedContacts: Contact[] = useMemo(() => {
    if (!isSearching) return contacts;
    return searchResults.filter((c) => selectedContactListIds.has(c.list_id));
  }, [isSearching, contacts, searchResults, selectedContactListIds]);

  const showLoading = useDeferredLoading(loading);
  const showSearchSpinner = useDeferredLoading(searching);

  // Flat list of {separator | contact} entries — separators carry
  // a contact-list header, contacts hold the row data. focusIndex
  // points at the *contact* index in `flatContacts`; separators
  // never receive focus.
  const { entries, flatContacts } = useMemo(() => {
    return buildEntries(displayedContacts, contactListById, contactLists, t);
  }, [displayedContacts, contactListById, contactLists, t]);

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

  // We deliberately do NOT push a focus-row announcement through
  // the live region. Each row's `aria-label` already carries
  // name + organisation + email + list, and the screen reader
  // announces that natively when `aria-activedescendant` moves.
  // A parallel `announce()` was duplicating that announcement
  // (native read followed by a polite-live-region read of the
  // exact same string a moment later) — the user heard every
  // navigation twice. Position info ("Eintrag 3 von 1967") used
  // to live in this announce, but with a 2000-entry GAL the
  // index is more noise than signal; users who care can read it
  // off the visual position. The TaskView keeps its own
  // analogous announce because its lists stay short and the
  // duplicate isn't perceptible at that scale.

  const optionId = (i: number) => `${headingId}-row-${i}`;

  // The listbox is always rendered (with a presentation `<li>`
  // placeholder when no rows exist) so the auto-focus ref
  // always resolves to a real element and screen-reader users
  // tabbing into the view immediately land on something they
  // can navigate. `aria-busy` toggles while the fetch is in
  // flight; combined with `aria-live` on the loading text the
  // user hears both signals.
  return (
    <section
      aria-labelledby={`${headingId}-heading`}
      aria-busy={showLoading || undefined}
      className="contacts-view"
    >
      <header className="contacts-view__header">
        <h2 id={`${headingId}-heading`} className="contacts-view__title">
          {t('views.contacts.heading')}
        </h2>
        <div className="contacts-view__actions">
          {/* Refresh button drives a manual sync pass. Disabled while
              a pass is in flight so accidental double-clicks don't
              queue up. The status text below the listbox renders
              the last-synced timestamp once a pass has completed. */}
          <button
            type="button"
            className="form__action"
            onClick={() => {
              // Omit the explicit `false` — defer to the user's
              // `contacts.includeReadOnlyOnSync` pref so the
              // Refresh button matches whatever the periodic
              // scheduler does. Without this, refreshing the
              // contacts list would silently skip GAL / Suggested
              // People even when the user had opted in.
              void triggerSync();
            }}
            disabled={triggering || syncStatus?.in_flight === true}
            aria-label={t('views.contacts.refreshAria')}
            title={t('views.contacts.refreshAria')}
          >
            {syncStatus?.in_flight || triggering
              ? t('views.contacts.refreshing')
              : t('views.contacts.refresh')}
          </button>
          <button
            type="button"
            className="form__action form__action--primary"
            onClick={openCreate}
          >
            {t('views.contacts.add')}
          </button>
        </div>
      </header>
      <p className="contacts-view__sync-status form__hint" aria-live="polite">
        {/* Render last-synced time once we have it; the polite live
            region lets screen readers pick up the change after each
            refresh without stealing focus. */}
        {syncStatus?.last_synced_at
          ? t('views.contacts.lastSynced', {
              time: formatLastSynced(
                syncStatus.last_synced_at,
                i18n.language,
              ),
            })
          : t('views.contacts.neverSynced')}
      </p>

      <div className="contacts-view__search">
        <label htmlFor={searchId} className="sr-only">
          {t('views.contacts.searchLabel')}
        </label>
        <input
          id={searchId}
          type="search"
          role="searchbox"
          value={searchInput}
          onChange={(e) => setSearchInput(e.target.value)}
          placeholder={t('views.contacts.searchPlaceholder')}
          aria-describedby={`${searchId}-hint`}
          autoComplete="off"
          spellCheck={false}
        />
        <span id={`${searchId}-hint`} className="form__hint">
          {t('views.contacts.searchHint')}
        </span>
      </div>

      {showSearchSpinner && (
        <p role="status" aria-live="polite" className="form__hint">
          {t('views.contacts.searching')}
        </p>
      )}

      {!showLoading && isSearching && flatContacts.length > 0 && (
        <p role="status" aria-live="polite" className="form__hint sr-only">
          {t('views.contacts.searchResults', { count: flatContacts.length })}
        </p>
      )}

      <ul
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.contacts.listLabel')}
        aria-activedescendant={
          listHasFocus && flatContacts.length > 0
            ? optionId(focusIndex)
            : undefined
        }
        onFocus={() => setListHasFocus(true)}
        onBlur={() => setListHasFocus(false)}
        onKeyDown={handleListKey}
        className="contacts-list"
      >
        {/* Empty / placeholder states live INSIDE the listbox
            so the `<ul>` is always the tab stop, even before
            the first contact arrives. Screen readers announce
            the listbox label, then the placeholder text. */}
        {showLoading && (
          <li role="presentation" className="contacts-list__placeholder">
            {t('views.contacts.loading')}
          </li>
        )}
        {!showLoading && flatContacts.length === 0 && (
          <li role="presentation" className="contacts-list__placeholder">
            {isSearching
              ? t('views.contacts.searchEmpty', { query: searchQuery })
              : t('views.contacts.empty')}
          </li>
        )}
        {!showLoading &&
          entries.map((entry) => {
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
                onClick={() => setFocusIndex(entry.index)}
                onDoubleClick={() => {
                  setFocusIndex(entry.index);
                  openEdit(entry.contact);
                }}
              >
                <span className="contacts-list__name">
                  {entry.contact.display_name}
                  {entry.contact.members !== null && (
                    // Distribution-list marker. We render a small
                    // pill next to the name rather than a separate
                    // row so the grouping stays inline with the
                    // alphabetised flow — same visual weight as
                    // the metadata line below.
                    <span
                      className="contacts-list__group-badge"
                      aria-label={t('views.contacts.groupBadgeAria', {
                        count: entry.contact.members.length,
                      })}
                    >
                      {t('views.contacts.groupBadge', {
                        count: entry.contact.members.length,
                      })}
                    </span>
                  )}
                </span>
                {(entry.contact.organization ||
                  entry.contact.emails.length > 0 ||
                  (entry.contact.members !== null &&
                    entry.contact.members.length > 0)) && (
                  <span className="contacts-list__meta">
                    {entry.contact.organization && (
                      <span className="contacts-list__org">
                        {entry.contact.organization}
                      </span>
                    )}
                    {entry.contact.members !== null
                      ? entry.contact.members[0] && (
                          <span className="contacts-list__email">
                            {entry.contact.members[0].name ??
                              entry.contact.members[0].email}
                            {entry.contact.members.length > 1 &&
                              ` +${entry.contact.members.length - 1}`}
                          </span>
                        )
                      : entry.contact.emails[0] && (
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
    </section>
  );
}

// ─────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────

/**
 * Format the last-synced timestamp for the panel footer. Recent
 * passes (under a minute ago) render as "gerade eben" / "just now"
 * — the wall-clock timestamp is too noisy when the user clicked
 * Refresh five seconds ago. Older values fall through to a locale
 * "today at 13:42" / "yesterday at 13:42" / full date+time
 * depending on how far back they reach. Pure helper so the call
 * site stays readable.
 */
function formatLastSynced(iso: string, language: string): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) {
    return iso;
  }
  const now = new Date();
  const deltaMs = now.getTime() - parsed.getTime();
  // Under a minute: collapse to a relative phrase that doesn't
  // tick — the polite live region would re-announce a ticking
  // value on every render, which gets old fast.
  if (deltaMs >= 0 && deltaMs < 60_000) {
    return new Intl.RelativeTimeFormat(language, { numeric: 'auto' }).format(
      0,
      'minute',
    );
  }
  const isSameDay =
    parsed.getFullYear() === now.getFullYear() &&
    parsed.getMonth() === now.getMonth() &&
    parsed.getDate() === now.getDate();
  if (isSameDay) {
    return parsed.toLocaleTimeString(language, {
      hour: '2-digit',
      minute: '2-digit',
    });
  }
  return parsed.toLocaleString(language, {
    dateStyle: 'long',
    timeStyle: 'short',
  });
}

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
        name: getContactListDisplayName(list, t),
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
  if (list)
    parts.push(
      t('views.contacts.fromList', { list: getContactListDisplayName(list, t) }),
    );
  return parts.join(', ');
}
