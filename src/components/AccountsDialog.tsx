import {
  useCallback,
  useEffect,
  useId,
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
import { useAutoFocus } from '../hooks/useAutoFocus';
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

  const formRef = useAutoFocus<HTMLInputElement>(!loading);
  const headingId = useId();

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

        <section aria-labelledby={`${headingId}-list`} className="accounts-list-section">
          <h3 id={`${headingId}-list`} className="form__label">
            {t('dialogs.accounts.existingHeading')}
          </h3>
          {accounts.length === 0 && !loading ? (
            <p className="form__hint">{t('dialogs.accounts.empty')}</p>
          ) : (
            <ul className="accounts-list" role="list">
              {accounts.map((acc) => {
                const isLocal = acc.adapter_kind === 'local' && acc.id === 'local';
                return (
                  <li
                    key={acc.id}
                    className="accounts-list__item"
                    aria-label={t('dialogs.accounts.rowLabel', {
                      name: acc.display_name,
                      kind: t(`dialogs.accounts.kindName.${acc.adapter_kind}`),
                    })}
                  >
                    <span className="accounts-list__name">
                      {acc.display_name}
                    </span>
                    <span className="accounts-list__kind">
                      {t(`dialogs.accounts.kindName.${acc.adapter_kind}`)}
                    </span>
                    <button
                      type="button"
                      className="form__action form__action--danger"
                      disabled={isLocal}
                      title={
                        isLocal
                          ? t('dialogs.accounts.localCannotDelete')
                          : undefined
                      }
                      onClick={() => setConfirmTarget(acc)}
                    >
                      {t('dialogs.accounts.delete')}
                    </button>
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
                ref={formRef}
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
