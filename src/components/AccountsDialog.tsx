import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createAccount,
  deleteAccount,
  isCommandError,
  listAccounts,
  testCaldavConnection,
  testIcalFeed,
} from '../api/client';
import type { Account, AdapterKind } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { ConfirmDialog } from './ConfirmDialog';
import { Modal } from './Modal';

/**
 * Account management dialog (DESIGN.md §6.2 / §6.4).
 *
 * Two halves stacked vertically:
 *   - top: existing accounts with a Delete button each
 *   - bottom: "add account" form
 *
 * Phase 6a only ships the local adapter. The kind picker shows every
 * adapter the spec lists, but the non-local entries are disabled with
 * a "coming soon" label so users see the roadmap directly in the UI.
 */
export interface AccountsDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

/** Order of the kind picker. Local first because it's the only
 *  enabled choice in Phase 6a; the others follow the rough release
 *  order announced in the spec. */
const KIND_ORDER: AdapterKind[] = [
  'local',
  'caldav',
  'ical',
  'google',
  'microsoft_graph',
  'ews',
  'vikunja',
  'todoist',
];

const ENABLED_KINDS: ReadonlySet<AdapterKind> = new Set([
  'local',
  'caldav',
  'ical',
]);

interface CaldavFields {
  serverUrl: string;
  username: string;
  password: string;
}

const EMPTY_CALDAV: CaldavFields = {
  serverUrl: '',
  username: '',
  password: '',
};

interface IcalFields {
  feedUrl: string;
  username: string;
  password: string;
}

const EMPTY_ICAL: IcalFields = {
  feedUrl: '',
  username: '',
  password: '',
};

export function AccountsDialog({ isOpen, onClose }: AccountsDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  // The store owns the calendar / task-list catalog the sidebar
  // renders from. We have to nudge it manually after creating or
  // deleting an account — otherwise the new account's calendars
  // wouldn't show up until something else triggers a store refresh.
  const { refreshCalendars, refreshTaskLists } = useCalendarStore();

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirmTarget, setConfirmTarget] = useState<Account | null>(null);

  const [kind, setKind] = useState<AdapterKind>('local');
  const [displayName, setDisplayName] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testMessage, setTestMessage] = useState<string | null>(null);
  const [caldav, setCaldav] = useState<CaldavFields>(EMPTY_CALDAV);
  const [ical, setIcal] = useState<IcalFields>(EMPTY_ICAL);

  const refresh = useCallback(() => {
    setLoading(true);
    setError(null);
    listAccounts()
      .then(setAccounts)
      .catch((err) => {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      })
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    if (isOpen) refresh();
  }, [isOpen, refresh]);

  const validateCaldav = useCallback((): string | null => {
    if (!caldav.serverUrl.trim()) return t('dialogs.accounts.serverUrlRequired');
    if (!caldav.username.trim()) return t('dialogs.accounts.usernameRequired');
    if (!caldav.password) return t('dialogs.accounts.passwordRequired');
    return null;
  }, [caldav, t]);

  const validateIcal = useCallback((): string | null => {
    // iCal only requires the URL — username + password are optional
    // (most public feeds are anonymous).
    if (!ical.feedUrl.trim()) return t('dialogs.accounts.feedUrlRequired');
    return null;
  }, [ical, t]);

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      setTestMessage(null);
      const name = displayName.trim();
      if (!name) {
        setError(t('dialogs.accounts.nameRequired'));
        return;
      }
      if (!ENABLED_KINDS.has(kind)) {
        // Defence in depth — UI should already block disabled kinds.
        setError(t('dialogs.accounts.kindUnavailable'));
        return;
      }
      if (kind === 'caldav') {
        const v = validateCaldav();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'ical') {
        const v = validateIcal();
        if (v) {
          setError(v);
          return;
        }
      }
      setSubmitting(true);
      setError(null);
      try {
        const configJson =
          kind === 'caldav'
            ? JSON.stringify({
                server_url: caldav.serverUrl.trim(),
                username: caldav.username.trim(),
                auth_kind: 'basic',
              })
            : kind === 'ical'
              ? JSON.stringify({
                  feed_url: ical.feedUrl.trim(),
                  username: ical.username.trim() || null,
                })
              : '{}';
        const secret =
          kind === 'caldav'
            ? caldav.password
            : kind === 'ical' && ical.password
              ? ical.password
              : undefined;
        const created = await createAccount({
          adapter_kind: kind,
          display_name: name,
          config_json: configJson,
          secret,
        });
        announce(t('dialogs.accounts.created', { name: created.display_name }));
        setDisplayName('');
        setCaldav(EMPTY_CALDAV);
        setIcal(EMPTY_ICAL);
        refresh();
        // Re-fetch the calendar / task-list catalog so the sidebar
        // picks up the new account's containers without the user
        // having to open another dialog (or restart the app). Errors
        // here are non-fatal — the account row was already
        // persisted; a stale sidebar fixes itself on the next normal
        // store refresh.
        void Promise.allSettled([refreshCalendars(), refreshTaskLists()]);
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      } finally {
        setSubmitting(false);
      }
    },
    [
      displayName,
      kind,
      caldav,
      ical,
      validateCaldav,
      validateIcal,
      announce,
      refresh,
      refreshCalendars,
      refreshTaskLists,
      t,
    ],
  );

  const onTestConnection = useCallback(async () => {
    setTestMessage(null);
    setError(null);
    setTesting(true);
    try {
      if (kind === 'caldav') {
        const v = validateCaldav();
        if (v) {
          setError(v);
          return;
        }
        await testCaldavConnection(
          caldav.serverUrl.trim(),
          caldav.username.trim(),
          caldav.password,
        );
      } else if (kind === 'ical') {
        const v = validateIcal();
        if (v) {
          setError(v);
          return;
        }
        await testIcalFeed(
          ical.feedUrl.trim(),
          ical.username.trim() || null,
          ical.password || null,
        );
      } else {
        return;
      }
      setTestMessage(t('dialogs.accounts.testOk'));
      announce(t('dialogs.accounts.testOk'));
    } catch (err) {
      if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
      else setError(String(err));
    } finally {
      setTesting(false);
    }
  }, [kind, caldav, ical, validateCaldav, validateIcal, announce, t]);

  const performDelete = useCallback(
    async (acc: Account) => {
      setError(null);
      try {
        await deleteAccount(acc.id);
        announce(t('dialogs.accounts.deleted', { name: acc.display_name }));
        refresh();
        // Same as on create: nudge the store so the just-removed
        // account's calendars disappear from the sidebar.
        void Promise.allSettled([refreshCalendars(), refreshTaskLists()]);
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      }
    },
    [announce, refresh, refreshCalendars, refreshTaskLists, t],
  );

  const headingId = useId();
  const optionId = (i: number) => `${headingId}-acc-${i}`;

  // Two-track focus model:
  //  - When the list has rows, the <ul> takes the tab stop and uses
  //    aria-activedescendant to spotlight the focused row. Arrow keys
  //    move the spotlight, Enter / Delete open the confirm dialog
  //    (no-op for the local row).
  //  - When the list is empty, the surrounding <section> takes the
  //    tab stop so the screen reader still hears the heading and the
  //    empty-state hint instead of skipping straight to the form.
  const sectionRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const [focusIndex, setFocusIndex] = useState(0);

  useEffect(() => {
    if (focusIndex >= accounts.length) {
      setFocusIndex(Math.max(0, accounts.length - 1));
    }
  }, [accounts.length, focusIndex]);

  // Land focus on the listbox (or the section fallback) once the
  // accounts have loaded, so the user can start arrow-navigating
  // immediately without first chasing a tab stop.
  useEffect(() => {
    if (!isOpen || loading) return;
    (listRef.current ?? sectionRef.current)?.focus({ preventScroll: true });
  }, [isOpen, loading, accounts.length]);

  const isLocalAt = (i: number): boolean => {
    const acc = accounts[i];
    return !!acc && acc.adapter_kind === 'local' && acc.id === 'local';
  };

  const tryDelete = useCallback(
    (i: number) => {
      const acc = accounts[i];
      if (!acc) return;
      if (acc.adapter_kind === 'local' && acc.id === 'local') {
        announce(t('dialogs.accounts.localCannotDelete'));
        return;
      }
      setConfirmTarget(acc);
    },
    [accounts, announce, t],
  );

  const handleListKey = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (accounts.length === 0) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setFocusIndex((i) => Math.min(i + 1, accounts.length - 1));
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
        setFocusIndex(accounts.length - 1);
        return;
      case 'Enter':
      case ' ':
      case 'Spacebar':
      case 'Delete':
      case 'Backspace':
        e.preventDefault();
        tryDelete(focusIndex);
        return;
      default:
        return;
    }
  };

  const sectionTabIndex = accounts.length === 0 && !loading ? 0 : -1;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.accounts.title')}
      className="modal--form"
    >
      <div className="form">
        <p
          className="sr-only"
          aria-live="polite"
          aria-atomic="true"
        >
          {loading
            ? t('dialogs.accounts.loading')
            : t('dialogs.accounts.count', { count: accounts.length })}
        </p>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <section
          ref={sectionRef}
          tabIndex={sectionTabIndex}
          aria-labelledby={`${headingId}-list`}
          aria-describedby={
            accounts.length === 0 ? `${headingId}-empty` : undefined
          }
          className="accounts-list-section"
        >
          <h3 id={`${headingId}-list`} className="form__label">
            {t('dialogs.accounts.existingHeading')}
          </h3>
          {accounts.length === 0 && !loading ? (
            <p id={`${headingId}-empty`} className="form__hint">
              {t('dialogs.accounts.empty')}
            </p>
          ) : (
            <ul
              ref={listRef}
              role="listbox"
              tabIndex={0}
              aria-label={t('dialogs.accounts.listLabel')}
              aria-activedescendant={
                accounts.length > 0 ? optionId(focusIndex) : undefined
              }
              onKeyDown={handleListKey}
              className="accounts-list"
            >
              {accounts.map((acc, i) => {
                const isLocal = isLocalAt(i);
                const focused = i === focusIndex;
                return (
                  <li
                    key={acc.id}
                    id={optionId(i)}
                    role="option"
                    aria-selected={focused}
                    aria-label={t(
                      isLocal
                        ? 'dialogs.accounts.rowLabelLocal'
                        : 'dialogs.accounts.rowLabel',
                      {
                        name: acc.display_name,
                        kind: t(`dialogs.accounts.kindName.${acc.adapter_kind}`),
                      },
                    )}
                    className={
                      'accounts-list__item' +
                      (focused ? ' accounts-list__item--focused' : '')
                    }
                    onClick={() => {
                      setFocusIndex(i);
                      if (!isLocal) setConfirmTarget(acc);
                    }}
                  >
                    <span className="accounts-list__name">
                      {acc.display_name}
                    </span>
                    <span className="accounts-list__kind">
                      {t(`dialogs.accounts.kindName.${acc.adapter_kind}`)}
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
        </section>

        <section aria-labelledby={`${headingId}-add`} className="accounts-add-section">
          <h3 id={`${headingId}-add`} className="form__label">
            {t('dialogs.accounts.addHeading')}
          </h3>
          <form onSubmit={onSubmit} className="form">
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.accounts.kindLabel')}
              </span>
              <select
                value={kind}
                onChange={(e) => setKind(e.target.value as AdapterKind)}
              >
                {KIND_ORDER.map((k) => (
                  <option
                    key={k}
                    value={k}
                    disabled={!ENABLED_KINDS.has(k)}
                  >
                    {t(`dialogs.accounts.kindName.${k}`)}
                    {!ENABLED_KINDS.has(k)
                      ? ` — ${t('dialogs.accounts.comingSoon')}`
                      : ''}
                  </option>
                ))}
              </select>
            </label>

            <label className="form__field">
              <span className="form__label">
                {t('dialogs.accounts.nameLabel')}
              </span>
              <input
                type="text"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder={t('dialogs.accounts.namePlaceholder')}
                autoComplete="off"
                required
              />
            </label>

            {kind === 'caldav' && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.serverUrlLabel')}
                  </span>
                  <input
                    type="url"
                    value={caldav.serverUrl}
                    onChange={(e) =>
                      setCaldav((prev) => ({
                        ...prev,
                        serverUrl: e.target.value,
                      }))
                    }
                    placeholder={t('dialogs.accounts.serverUrlPlaceholder')}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.serverUrlHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.usernameLabel')}
                  </span>
                  <input
                    type="text"
                    value={caldav.username}
                    onChange={(e) =>
                      setCaldav((prev) => ({
                        ...prev,
                        username: e.target.value,
                      }))
                    }
                    autoComplete="username"
                    spellCheck={false}
                    required
                  />
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.passwordLabel')}
                  </span>
                  <input
                    type="password"
                    value={caldav.password}
                    onChange={(e) =>
                      setCaldav((prev) => ({
                        ...prev,
                        password: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.passwordHint')}
                  </span>
                </label>
                {testMessage && kind === 'caldav' && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            {kind === 'ical' && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.feedUrlLabel')}
                  </span>
                  <input
                    type="url"
                    value={ical.feedUrl}
                    onChange={(e) =>
                      setIcal((prev) => ({
                        ...prev,
                        feedUrl: e.target.value,
                      }))
                    }
                    placeholder={t('dialogs.accounts.feedUrlPlaceholder')}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.feedUrlHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.icalUsernameLabel')}
                  </span>
                  <input
                    type="text"
                    value={ical.username}
                    onChange={(e) =>
                      setIcal((prev) => ({
                        ...prev,
                        username: e.target.value,
                      }))
                    }
                    autoComplete="username"
                    spellCheck={false}
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.icalAuthHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.icalPasswordLabel')}
                  </span>
                  <input
                    type="password"
                    value={ical.password}
                    onChange={(e) =>
                      setIcal((prev) => ({
                        ...prev,
                        password: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                  />
                </label>
                {testMessage && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            <div className="form__actions">
              {(kind === 'caldav' || kind === 'ical') && (
                <button
                  type="button"
                  className="form__action"
                  onClick={onTestConnection}
                  aria-disabled={testing || submitting || undefined}
                >
                  {testing
                    ? t('dialogs.accounts.testing')
                    : t('dialogs.accounts.testConnection')}
                </button>
              )}
              <button
                type="submit"
                className="form__action form__action--primary"
                aria-disabled={
                  submitting || testing || !ENABLED_KINDS.has(kind) || undefined
                }
              >
                {t('dialogs.accounts.add')}
              </button>
            </div>
          </form>
        </section>

        <div className="form__actions">
          <button type="button" onClick={onClose} className="form__action">
            {t('dialogs.close')}
          </button>
        </div>
      </div>

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget);
        }}
        title={t('dialogs.accounts.deleteTitle')}
        message={t('dialogs.accounts.deleteMessage', {
          name: confirmTarget?.display_name ?? '',
        })}
      />
    </Modal>
  );
}
