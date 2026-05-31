import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  isCommandError,
  reconnectGoogleAccount,
  reconnectMicrosoftAccount,
  setAccountSecret,
} from '../api/client';
import type { Account, AdapterKind } from '../api/types';
import { Modal } from './Modal';

/**
 * "Konten verbinden" wizard (DESIGN.md §19.11 step 8).
 *
 * After a successful `accept_remote_dataset` on a fresh device,
 * the snapshot has populated the `accounts` table — but the OS
 * keychain on this device is empty, so every external adapter is
 * unreachable until the user re-attaches credentials. Secrets are
 * device-local by design (§19.2.1) and intentionally never
 * traverse the sync store.
 *
 * The dialog renders one row per missing account. The row form
 * branches on `adapter_kind`:
 *   - **password-based** (CalDAV, iCal-with-auth, EWS, Vikunja,
 *     Todoist): inline password input + "Save" → `setAccountSecret`.
 *     The single-slot model is enough for these — the dialog
 *     reuses the per-kind label ("API token" / "password") so the
 *     placeholder copy stays accurate.
 *   - **OAuth** (Google, Microsoft Graph): a single "Sign in"
 *     button → `reconnectGoogleAccount` / `reconnectMicrosoftAccount`.
 *     Both commands open the system browser and block until the
 *     callback round-trips.
 *
 * Per-row success removes the row from the list. When the list
 * empties the dialog closes itself (the user is done). The
 * "Later" button at the bottom closes the dialog without
 * touching pending rows — the next sync round will surface the
 * affected accounts as `auth` failures so the user has a path
 * back to this same wizard via the Settings panel.
 */
export interface SyncAccountsConnectDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Pre-fetched list of accounts the dialog should walk. The
   *  caller (SyncPanel) reads this from
   *  `listAccountsMissingCredentials` so the wizard only opens
   *  when there's actually something to do. */
  accounts: Account[];
}

type RowStatus =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'error'; message: string };

/** OAuth kinds use a dedicated reconnect command (no inline
 *  password input). Everything else falls through to the generic
 *  `setAccountSecret` path. */
function isOAuthKind(kind: AdapterKind): boolean {
  return kind === 'google' || kind === 'microsoft_graph';
}

export function SyncAccountsConnectDialog({
  isOpen,
  onClose,
  accounts: initialAccounts,
}: SyncAccountsConnectDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  // Local working copy: rows disappear from the list once their
  // reconnect succeeds. Seeded from the prop on mount; subsequent
  // updates to the prop are ignored — the wizard is a snapshot of
  // the state when it was opened, not a live view.
  const [pending, setPending] = useState<Account[]>(initialAccounts);
  const [rowStatus, setRowStatus] = useState<Record<string, RowStatus>>({});
  // Per-row password input state. Keyed by account id so each
  // row keeps its own value while the user types.
  const [passwordInputs, setPasswordInputs] = useState<Record<string, string>>(
    {},
  );

  // Re-seed when the dialog opens with a new account list — the
  // caller passes a fresh array each time, so a referential
  // comparison is enough to detect "fresh open".
  useEffect(() => {
    setPending(initialAccounts);
    setRowStatus({});
    setPasswordInputs({});
  }, [initialAccounts]);

  // Close automatically once the user has worked through every
  // row. The empty-list check after the initial open is the
  // canonical "wizard done" signal.
  useEffect(() => {
    if (isOpen && pending.length === 0) {
      // Defer the close so the success announcement reaches the
      // live region before the dialog unmounts.
      const handle = window.setTimeout(() => {
        onClose();
      }, 50);
      return () => window.clearTimeout(handle);
    }
    return undefined;
  }, [isOpen, onClose, pending.length]);

  const setStatus = useCallback((accountId: string, status: RowStatus) => {
    setRowStatus((prev) => ({ ...prev, [accountId]: status }));
  }, []);

  const removeRow = useCallback((accountId: string) => {
    setPending((prev) => prev.filter((row) => row.id !== accountId));
    setRowStatus((prev) => {
      const next = { ...prev };
      delete next[accountId];
      return next;
    });
    setPasswordInputs((prev) => {
      const next = { ...prev };
      delete next[accountId];
      return next;
    });
  }, []);

  // Translate a thrown error from the backend reconnect commands
  // into a short user-visible message. We keep the original
  // `message` text for codes we don't recognise so the user has
  // something to act on rather than a generic "failed".
  const messageForError = useCallback(
    (err: unknown): string => {
      if (isCommandError(err)) {
        switch (err.code) {
          case 'auth':
            return t('syncAccountsConnect.errorAuth');
          case 'network':
            return t('syncAccountsConnect.errorNetwork');
          case 'invalid_input':
            return t('syncAccountsConnect.errorInvalidInput');
          default:
            return err.message;
        }
      }
      return err instanceof Error ? err.message : String(err);
    },
    [t],
  );

  const onSavePassword = useCallback(
    async (account: Account) => {
      const secret = (passwordInputs[account.id] ?? '').trim();
      if (!secret) {
        setStatus(account.id, {
          kind: 'error',
          message: t('syncAccountsConnect.errorEmpty'),
        });
        return;
      }
      setStatus(account.id, { kind: 'busy' });
      try {
        await setAccountSecret(account.id, secret);
        announce(
          t('syncAccountsConnect.connectedAnnouncement', {
            name: account.display_name,
          }),
        );
        removeRow(account.id);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set_account_secret failed', err);
        setStatus(account.id, {
          kind: 'error',
          message: messageForError(err),
        });
      }
    },
    [announce, messageForError, passwordInputs, removeRow, setStatus, t],
  );

  const onOAuthSignIn = useCallback(
    async (account: Account) => {
      setStatus(account.id, { kind: 'busy' });
      try {
        if (account.adapter_kind === 'google') {
          await reconnectGoogleAccount(account.id);
        } else if (account.adapter_kind === 'microsoft_graph') {
          await reconnectMicrosoftAccount(account.id);
        } else {
          throw new Error(`unexpected OAuth kind: ${account.adapter_kind}`);
        }
        announce(
          t('syncAccountsConnect.connectedAnnouncement', {
            name: account.display_name,
          }),
        );
        removeRow(account.id);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('reconnect_oauth_account failed', err);
        setStatus(account.id, {
          kind: 'error',
          message: messageForError(err),
        });
      }
    },
    [announce, messageForError, removeRow, setStatus, t],
  );

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('syncAccountsConnect.title')}
      className="sync-accounts-connect"
      // The wizard is dismissible — "Later" is a legitimate
      // choice — but the backdrop tap is too easy to hit when
      // the dialog is doing real work. Force the conscious
      // gesture via the explicit close button.
      dismissOnBackdrop={false}
    >
      <FocusableNote className="sync-accounts-connect__body">
        {t('syncAccountsConnect.body')}
      </FocusableNote>
      <FocusableNote className="sync-accounts-connect__hint">
        {t('syncAccountsConnect.hint')}
      </FocusableNote>
      {pending.length === 0 ? (
        <FocusableNote className="sync-accounts-connect__empty">
          {t('syncAccountsConnect.empty')}
        </FocusableNote>
      ) : (
        <ul className="sync-accounts-connect__list">
          {pending.map((account) => (
            <SyncAccountsConnectRow
              key={account.id}
              account={account}
              status={rowStatus[account.id] ?? { kind: 'idle' }}
              password={passwordInputs[account.id] ?? ''}
              onPasswordChange={(value) =>
                setPasswordInputs((prev) => ({
                  ...prev,
                  [account.id]: value,
                }))
              }
              onSavePassword={() => void onSavePassword(account)}
              onOAuthSignIn={() => void onOAuthSignIn(account)}
            />
          ))}
        </ul>
      )}
      <div className="sync-accounts-connect__actions">
        <button type="button" onClick={onClose}>
          {pending.length === 0
            ? t('syncAccountsConnect.actionDone')
            : t('syncAccountsConnect.actionLater')}
        </button>
      </div>
    </Modal>
  );
}

interface RowProps {
  account: Account;
  status: RowStatus;
  password: string;
  onPasswordChange: (value: string) => void;
  onSavePassword: () => void;
  onOAuthSignIn: () => void;
}

function SyncAccountsConnectRow({
  account,
  status,
  password,
  onPasswordChange,
  onSavePassword,
  onOAuthSignIn,
}: RowProps) {
  const { t } = useTranslation();
  const inputId = useId();
  const isBusy = status.kind === 'busy';
  const isOAuth = isOAuthKind(account.adapter_kind);

  const kindLabel = useMemo(() => {
    return t(`syncAccountsConnect.kind.${account.adapter_kind}`, {
      defaultValue: account.adapter_kind,
    });
  }, [account.adapter_kind, t]);

  const secretLabel = useMemo(() => {
    // Token vs password is purely a UI distinction; both go to
    // the same backend command. Picking the right label keeps the
    // copy honest — "password" for an API token would mislead.
    switch (account.adapter_kind) {
      case 'vikunja':
      case 'todoist':
        return t('syncAccountsConnect.secretLabelToken');
      default:
        return t('syncAccountsConnect.secretLabelPassword');
    }
  }, [account.adapter_kind, t]);

  return (
    <li className="sync-accounts-connect__row">
      <div className="sync-accounts-connect__row-head">
        <strong className="sync-accounts-connect__row-name">
          {account.display_name}
        </strong>
        <span className="sync-accounts-connect__row-kind">{kindLabel}</span>
      </div>
      {isOAuth ? (
        <div className="sync-accounts-connect__row-action">
          <p className="sync-accounts-connect__row-hint">
            {t('syncAccountsConnect.oauthHint')}
          </p>
          <button type="button" disabled={isBusy} onClick={onOAuthSignIn}>
            {isBusy
              ? t('syncAccountsConnect.signingIn')
              : t('syncAccountsConnect.actionSignIn')}
          </button>
        </div>
      ) : (
        <div className="sync-accounts-connect__row-action">
          <label htmlFor={inputId}>{secretLabel}</label>
          <input
            id={inputId}
            type="password"
            value={password}
            onChange={(e) => onPasswordChange(e.target.value)}
            autoComplete="new-password"
            disabled={isBusy}
          />
          <button
            type="button"
            disabled={isBusy || password.trim().length === 0}
            onClick={onSavePassword}
          >
            {isBusy
              ? t('syncAccountsConnect.saving')
              : t('syncAccountsConnect.actionSave')}
          </button>
        </div>
      )}
      {status.kind === 'error' && (
        <p className="sync-accounts-connect__row-error" role="alert">
          {status.message}
        </p>
      )}
    </li>
  );
}
