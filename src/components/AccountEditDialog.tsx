import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { collectValues, firstMissingField } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  accountFormSpec,
  getUserPref,
  runAccountAction,
  testAccount,
  updateAccount,
  type AccountFormSpec,
} from '../api/client';
import type { Account } from '../api/types';
import { AccountSchemaForm } from './AccountSchemaForm';
import { Modal } from './Modal';

/**
 * Edit an existing account — server URL, endpoint, username, credentials —
 * on the SAME schema-driven form the add flow renders, prefilled from the
 * stored config (and the device-local half). Two edit-specific rules:
 *
 *   - Secret fields come back BLANK and blank means "keep what is stored"
 *     (the backend inherits the keychain value — see
 *     `host_core::account_update`), so the form's required-marks on secrets
 *     are lifted and a hint says what blank does.
 *   - The OAuth client pair is not offered: swapping the client invalidates
 *     the stored tokens, so that stays the reconnect flow's job.
 *
 * Saving travels: the config change goes out as `account.updated`, a changed
 * credential as an E2E `credential.set` — other devices pick both up and
 * rebuild their adapter from the new config.
 */
export function AccountEditDialog({
  isOpen,
  onClose,
  account,
  onSaved,
}: {
  isOpen: boolean;
  onClose: () => void;
  account: Account;
  /** Called after a successful save (the panel reloads its list). */
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const [spec, setSpec] = useState<AccountFormSpec | null>(null);
  const [values, setValues] = useState<Record<string, string | boolean>>({});
  const [displayName, setDisplayName] = useState(account.display_name);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  // The edit variant of the spec: the OAuth client pair is dropped, and
  // secret fields lose their required-mark (blank = keep the stored value).
  const editSpec = useMemo(() => {
    if (!spec) return null;
    const oauthKeys = new Set(
      [spec.oauth?.client_id_field, spec.oauth?.client_secret_field].filter(
        (k): k is string => k != null,
      ),
    );
    return {
      ...spec,
      // No oauth block: saving never opens a browser, so the schema form's
      // sign-in hints would be lies here; without the block its
      // builtin-optional exemptions have nothing to exempt either.
      oauth: null,
      fields: spec.fields
        .filter((f) => !oauthKeys.has(f.key))
        .map((f) => (f.kind === 'secret' ? { ...f, required: false } : f)),
    };
  }, [spec]);

  // Fetch the schema + seed the form from the stored halves whenever the
  // dialog opens: non-secret config values verbatim, device-local fields from
  // their prefs slot, secrets deliberately blank.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setError(null);
    setTestResult(null);
    setDisplayName(account.display_name);
    void (async () => {
      try {
        const fetched = await accountFormSpec(account.adapter_kind);
        if (cancelled) return;
        if (fetched == null) {
          // No plugin serves this kind (the Edit button is gated on the kind
          // being known, so this is a race with a plugin being disabled).
          setError(t('dialogs.accounts.pluginMissing'));
          return;
        }
        let config: Record<string, unknown> = {};
        try {
          config = JSON.parse(account.config_json) as Record<string, unknown>;
        } catch {
          // An unreadable config seeds an empty form; saving rewrites it.
        }
        const seeded: Record<string, string | boolean> = {};
        for (const field of fetched.fields) {
          if (field.kind === 'secret') continue;
          if (field.device_local) {
            const raw = await getUserPref(`account.${account.id}.${field.key}`);
            if (raw != null) {
              try {
                const parsed: unknown = JSON.parse(raw);
                if (typeof parsed === 'boolean') seeded[field.key] = parsed;
                else if (parsed != null) seeded[field.key] = String(parsed as string | number);
              } catch {
                seeded[field.key] = raw;
              }
            }
            continue;
          }
          const held = config[field.key];
          if (typeof held === 'boolean') seeded[field.key] = held;
          else if (held != null) seeded[field.key] = String(held as string | number);
        }
        if (cancelled) return;
        setSpec(fetched);
        setValues(seeded);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [isOpen, account, t]);

  const onChange = useCallback((key: string, value: string | boolean) => {
    setValues((prev) => ({ ...prev, [key]: value }));
    setTestResult(null);
  }, []);

  const validate = useCallback((): boolean => {
    if (!editSpec) return false;
    const missing = firstMissingField(editSpec, values);
    if (missing) {
      setError(t('dialogs.accounts.fieldRequired', { field: missing.label }));
      return false;
    }
    setError(null);
    return true;
  }, [editSpec, values, t]);

  const onTest = useCallback(async () => {
    if (!editSpec || !validate()) return;
    setTesting(true);
    setTestResult(null);
    try {
      await testAccount(account.adapter_kind, collectValues(editSpec, values), account.id);
      setTestResult(t('dialogs.accounts.testOk'));
      announce(t('dialogs.accounts.testOk'));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setTestResult(message);
      announce(message);
    } finally {
      setTesting(false);
    }
  }, [editSpec, values, account, validate, announce, t]);

  // One button per action the adapter declared (EWS Autodiscover) — the
  // same block the add form renders; editing an endpoint is exactly when a
  // discovery helper earns its keep.
  const [runningAction, setRunningAction] = useState<string | null>(null);
  const onAction = useCallback(
    async (actionKey: string) => {
      if (!editSpec) return;
      setRunningAction(actionKey);
      setError(null);
      try {
        const produced = await runAccountAction(
          account.adapter_kind,
          actionKey,
          collectValues(editSpec, values),
        );
        setValues((prev) => ({ ...prev, ...produced }));
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setRunningAction(null);
      }
    },
    [editSpec, values, account.adapter_kind],
  );

  const onSubmit = useCallback(async () => {
    if (!editSpec || busy) return;
    if (!displayName.trim()) {
      setError(t('dialogs.accounts.nameRequired'));
      return;
    }
    if (!validate()) return;
    setBusy(true);
    try {
      await updateAccount({
        account_id: account.id,
        display_name: displayName.trim(),
        values: collectValues(editSpec, values),
      });
      announce(t('dialogs.accounts.editSaved', { name: displayName.trim() }));
      onSaved();
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [editSpec, values, displayName, account.id, busy, validate, announce, onSaved, onClose, t]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.accounts.editTitle', { name: account.display_name })}
    >
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          void onSubmit();
        }}
      >
        {error && (
          <p className="form__error" role="alert">
            {error}
          </p>
        )}
        <label className="form__field">
          <span className="form__label">{t('dialogs.accounts.nameLabel')}</span>
          <input
            type="text"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            autoComplete="off"
            required
          />
        </label>
        {editSpec ? (
          <>
            <p className="form__hint">{t('dialogs.accounts.editSecretsHint')}</p>
            <AccountSchemaForm spec={editSpec} values={values} onChange={onChange} />
          </>
        ) : error == null ? (
          <p className="form__hint">{t('views.loading')}</p>
        ) : null}
        {testResult && (
          <p className="form__hint" role="status">
            {testResult}
          </p>
        )}
        <div className="form__actions">
          {(editSpec?.actions ?? []).map((action) => (
            <button
              key={action.key}
              type="button"
              className="form__action"
              aria-disabled={runningAction != null || undefined}
              onClick={() => {
                if (runningAction == null) void onAction(action.key);
              }}
            >
              {runningAction === action.key && action.busy_label
                ? action.busy_label
                : action.label}
            </button>
          ))}
          {editSpec?.supports_credential_test && (
            <button
              type="button"
              className="form__action"
              aria-disabled={testing || undefined}
              onClick={() => {
                if (!testing) void onTest();
              }}
            >
              {testing
                ? t('dialogs.accounts.testing')
                : t('dialogs.accounts.testConnection')}
            </button>
          )}
          <button
            type="submit"
            className="form__action"
            aria-disabled={busy || !editSpec || undefined}
          >
            {busy ? t('dialogs.accounts.editSaving') : t('dialogs.save')}
          </button>
          <button type="button" className="form__action" onClick={onClose}>
            {t('dialogs.confirm.cancel')}
          </button>
        </div>
      </form>
    </Modal>
  );
}
