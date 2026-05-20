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
} from '../api/client';
import type { Account, AdapterKind } from '../api/types';
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

const ENABLED_KINDS: ReadonlySet<AdapterKind> = new Set(['local']);

export function AccountsDialog({ isOpen, onClose }: AccountsDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirmTarget, setConfirmTarget] = useState<Account | null>(null);

  const [kind, setKind] = useState<AdapterKind>('local');
  const [displayName, setDisplayName] = useState('');
  const [submitting, setSubmitting] = useState(false);

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

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
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
      setSubmitting(true);
      setError(null);
      try {
        const created = await createAccount({
          adapter_kind: kind,
          display_name: name,
        });
        announce(t('dialogs.accounts.created', { name: created.display_name }));
        setDisplayName('');
        refresh();
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      } finally {
        setSubmitting(false);
      }
    },
    [displayName, kind, announce, refresh, t],
  );

  const performDelete = useCallback(
    async (acc: Account) => {
      setError(null);
      try {
        await deleteAccount(acc.id);
        announce(t('dialogs.accounts.deleted', { name: acc.display_name }));
        refresh();
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      }
    },
    [announce, refresh, t],
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

            <div className="form__actions">
              <button
                type="submit"
                className="form__action form__action--primary"
                disabled={submitting || !ENABLED_KINDS.has(kind)}
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
