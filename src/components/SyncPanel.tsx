import { useCallback, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  acceptRemoteDataset,
  adoptLocalDataset,
  compactNow,
  configureSyncAdapter,
  isCommandError,
  previewSyncTarget,
  setSyncInterval,
  type SyncAdapterConfig,
  type SyncPreview,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/DialogState';
import { useSync } from '../state/useSync';

/**
 * Settings → Synchronisation panel (DESIGN.md §19, Phase Si).
 *
 * Four sections:
 *
 *   1. **State** — connection state + last successful sync + manual
 *      `Sync now` / `Compact now`.
 *   2. **Interval** — slider/select over a preset list, defaulting
 *      to 5 minutes per §19.8.
 *   3. **Adapter** — kind picker + path input + Connect / Disconnect.
 *   4. **Onboarding** — preview button + accept/adopt buttons when
 *      a preview has run. Only visible while no adapter is
 *      configured (otherwise the user is already onboarded).
 *
 * The conflicts dialog isn't rendered inline — it's a separate
 * modal reachable from the status badge + a button at the bottom.
 */

const INTERVAL_PRESETS: readonly number[] = [1, 5, 15, 30, 60, 240];

export function SyncPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const { openSyncConflicts } = useDialogState();
  const {
    status,
    lastReport,
    lastError,
    conflictCount,
    triggering,
    triggerSync,
  } = useSync();

  const stateHeadingId = useId();
  const adapterHeadingId = useId();
  const intervalHeadingId = useId();
  const previewHeadingId = useId();

  // Adapter draft state. Seeded from current backend state on mount
  // so the inputs reflect the persisted choice.
  const [kindDraft, setKindDraft] = useState<
    'local' | 'webdav' | 'sftp' | 'none'
  >('local');
  const [pathDraft, setPathDraft] = useState('');
  // WebDAV-only fields. `passwordDraft` is empty on first render —
  // the persisted password lives in the OS keychain and we don't
  // surface it back. Submitting with an empty password reuses the
  // keychain entry.
  const [urlDraft, setUrlDraft] = useState('');
  const [userDraft, setUserDraft] = useState('');
  const [passwordDraft, setPasswordDraft] = useState('');
  // SFTP-only fields. `sftpPortDraft` is a string so the input
  // doesn't fight with React on every keystroke; we parse to u16
  // at submit time. Default port 22 is filled in on first render.
  const [sftpHostDraft, setSftpHostDraft] = useState('');
  const [sftpPortDraft, setSftpPortDraft] = useState('22');
  const [sftpUserDraft, setSftpUserDraft] = useState('');
  const [sftpPathDraft, setSftpPathDraft] = useState('');
  const [sftpPasswordDraft, setSftpPasswordDraft] = useState('');
  // Phase Sm-2: auth method radio + SSH-key fields. Password is
  // the default. The two key inputs only render when
  // `sftpAuthDraft === 'key'`.
  const [sftpAuthDraft, setSftpAuthDraft] = useState<'password' | 'key'>(
    'password',
  );
  const [sftpKeyPathDraft, setSftpKeyPathDraft] = useState('');
  const [sftpKeyPassphraseDraft, setSftpKeyPassphraseDraft] = useState('');
  // Phase Sk: E2E passphrase. Two roles depending on the
  // onboarding branch:
  //   - `adopt_local` with non-empty value → mints a fresh dataset
  //     with E2E enabled.
  //   - `accept_remote` against a preview that says `e2e_enabled`
  //     → the passphrase the user types to unlock the existing
  //     dataset's key.
  // `enableE2eDraft` is the explicit "I want to turn on
  // encryption for the new dataset" checkbox, used only on
  // adopt_local.
  const [passphraseDraft, setPassphraseDraft] = useState('');
  const [enableE2eDraft, setEnableE2eDraft] = useState(false);
  const [deviceNameDraft, setDeviceNameDraft] = useState('');
  const [intervalDraft, setIntervalDraft] = useState<number | null>(null);
  const [busyAdapter, setBusyAdapter] = useState(false);
  const [busyPreview, setBusyPreview] = useState(false);
  const [busyAccept, setBusyAccept] = useState(false);
  const [busyAdopt, setBusyAdopt] = useState(false);
  const [busyCompact, setBusyCompact] = useState(false);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  const interval = intervalDraft ?? status?.interval_minutes ?? 5;

  // Translate a `CommandError`-shaped error into a frontend message
  // keyed off the stable `code`. Falls back to the raw message when
  // the code is unknown so we never silently swallow context.
  const messageForError = useCallback(
    (err: unknown): string => {
      if (isCommandError(err)) {
        switch (err.code) {
          case 'auth':
            // SFTP host-key mismatch is surfaced as `auth` with a
            // distinctive message prefix — promote it to a
            // dedicated warning so the user sees the §19.5
            // verify-out-of-band guidance instead of a generic
            // "auth failed" string.
            if (err.message.includes('host key mismatch')) {
              return t(
                'dialogs.settings.sync.adapterSftpHostKeyMismatch',
              );
            }
            return t('dialogs.settings.sync.errorAuth');
          case 'io':
            return t('dialogs.settings.sync.errorIo');
          case 'network':
            return t('dialogs.settings.sync.errorNetwork');
          case 'not_found':
            return t('dialogs.settings.sync.errorNotFound');
          case 'encryption_required':
            return t('dialogs.settings.sync.errorEncryption');
          case 'schema_too_old':
            return t('dialogs.settings.sync.errorSchemaTooOld');
          default:
            return err.message;
        }
      }
      return err instanceof Error ? err.message : String(err);
    },
    [t],
  );

  const buildConfig = useCallback((): SyncAdapterConfig => {
    if (kindDraft === 'local') return { kind: 'local', path: pathDraft.trim() };
    if (kindDraft === 'webdav') {
      return {
        kind: 'webdav',
        url: urlDraft.trim(),
        user: userDraft.trim(),
        // Empty string → backend reuses keychain entry. Sending
        // `undefined` would serialize to `null` via serde's default;
        // either form is fine on the wire.
        password: passwordDraft.trim() || null,
      };
    }
    if (kindDraft === 'sftp') {
      // Parse the port; fall back to 22 on garbage. Backend
      // re-validates so anything we miss surfaces as
      // `invalid_input`.
      const port = Number.parseInt(sftpPortDraft, 10);
      return {
        kind: 'sftp',
        host: sftpHostDraft.trim(),
        port: Number.isFinite(port) && port > 0 ? port : 22,
        user: sftpUserDraft.trim(),
        path: sftpPathDraft.trim(),
        auth_method: sftpAuthDraft,
        // Only one side of the password vs key fields is
        // populated per round-trip; the unused fields go as
        // null. The backend ignores the inactive side.
        password:
          sftpAuthDraft === 'password'
            ? sftpPasswordDraft.trim() || null
            : null,
        key_path:
          sftpAuthDraft === 'key' ? sftpKeyPathDraft.trim() || null : null,
        key_passphrase:
          sftpAuthDraft === 'key'
            ? sftpKeyPassphraseDraft.trim() || null
            : null,
      };
    }
    return { kind: 'none' };
  }, [
    kindDraft,
    pathDraft,
    urlDraft,
    userDraft,
    passwordDraft,
    sftpHostDraft,
    sftpPortDraft,
    sftpUserDraft,
    sftpPathDraft,
    sftpPasswordDraft,
    sftpAuthDraft,
    sftpKeyPathDraft,
    sftpKeyPassphraseDraft,
  ]);

  // Validation: the Connect button needs a path for `local`, a URL +
  // user for `webdav`, and host + user + path for `sftp`. Per-kind
  // feedback steers the user before they hit the backend's error
  // code path.
  const configMissingRequired = (() => {
    if (kindDraft === 'local') return !pathDraft.trim();
    if (kindDraft === 'webdav') return !urlDraft.trim() || !userDraft.trim();
    if (kindDraft === 'sftp') {
      if (
        !sftpHostDraft.trim() ||
        !sftpUserDraft.trim() ||
        !sftpPathDraft.trim()
      ) {
        return true;
      }
      // Key auth needs a path on first connect; subsequent edits
      // reuse the previously-saved path so the empty-but-
      // configured case stays valid.
      if (sftpAuthDraft === 'key' && !sftpKeyPathDraft.trim() && !status?.configured) {
        return true;
      }
      return false;
    }
    return false;
  })();

  const onIntervalChange = useCallback(
    async (raw: string) => {
      const parsed = Number(raw);
      if (!Number.isFinite(parsed) || parsed < 1) return;
      setIntervalDraft(parsed);
      try {
        const persisted = await setSyncInterval(parsed);
        setIntervalDraft(persisted);
        announce(
          t('dialogs.settings.sync.intervalChanged', { minutes: persisted }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set_sync_interval failed', err);
        // Restore the previous value so the select doesn't lie.
        setIntervalDraft(status?.interval_minutes ?? 5);
      }
    },
    [announce, status?.interval_minutes, t],
  );

  const onConfigure = useCallback(async () => {
    if (configMissingRequired) {
      announce(t('dialogs.settings.sync.adapterNeedPath'), 'assertive');
      return;
    }
    setBusyAdapter(true);
    try {
      await configureSyncAdapter(buildConfig());
      // Clear password fields after a successful connect so they
      // don't sit in memory longer than necessary. The keychain
      // entry is the canonical store from this point on.
      if (kindDraft === 'webdav') setPasswordDraft('');
      if (kindDraft === 'sftp') {
        setSftpPasswordDraft('');
        setSftpKeyPassphraseDraft('');
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('configure_sync_adapter failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyAdapter(false);
    }
  }, [
    announce,
    buildConfig,
    configMissingRequired,
    kindDraft,
    messageForError,
    t,
  ]);

  const onDisconnect = useCallback(async () => {
    setBusyAdapter(true);
    try {
      await configureSyncAdapter({ kind: 'none' });
      setPreview(null);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('configure_sync_adapter(none) failed', err);
    } finally {
      setBusyAdapter(false);
    }
  }, []);

  const onPreview = useCallback(async () => {
    if (kindDraft === 'none' || configMissingRequired) {
      announce(t('dialogs.settings.sync.adapterNeedPath'), 'assertive');
      return;
    }
    setBusyPreview(true);
    setPreviewError(null);
    try {
      const result = await previewSyncTarget(buildConfig());
      setPreview(result);
    } catch (err) {
      setPreviewError(messageForError(err));
      setPreview(null);
    } finally {
      setBusyPreview(false);
    }
  }, [
    announce,
    buildConfig,
    configMissingRequired,
    kindDraft,
    messageForError,
    t,
  ]);

  const onAccept = useCallback(async () => {
    setBusyAccept(true);
    try {
      const report = await acceptRemoteDataset(
        buildConfig(),
        deviceNameDraft.trim() || null,
        passphraseDraft.trim() || null,
      );
      const message =
        report.device_count === 1
          ? t('dialogs.settings.sync.onboardingDone_one')
          : t('dialogs.settings.sync.onboardingDone_other', {
              count: report.device_count,
            });
      announce(message);
      setPreview(null);
      // Clear the passphrase after a successful onboarding; the
      // derived key lives in the keychain from this point on.
      setPassphraseDraft('');
    } catch (err) {
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyAccept(false);
    }
  }, [
    announce,
    buildConfig,
    deviceNameDraft,
    messageForError,
    passphraseDraft,
    t,
  ]);

  const onAdopt = useCallback(async () => {
    const confirmed = window.confirm(t('dialogs.settings.sync.previewAdoptConfirm'));
    if (!confirmed) return;
    // Phase Sk: if the user ticked "Enable encryption", a
    // passphrase is required. Empty + ticked is a dead-end
    // configuration; gate before hitting the backend.
    if (enableE2eDraft && !passphraseDraft.trim()) {
      announce(
        t('dialogs.settings.sync.e2ePassphraseRequired'),
        'assertive',
      );
      return;
    }
    setBusyAdopt(true);
    try {
      const report = await adoptLocalDataset(
        buildConfig(),
        deviceNameDraft.trim() || null,
        enableE2eDraft ? passphraseDraft.trim() : null,
      );
      const message = report.remote_was_empty
        ? t('dialogs.settings.sync.onboardingFresh')
        : t('dialogs.settings.sync.onboardingDone_one');
      announce(message);
      setPreview(null);
      // Clear the passphrase after a successful onboarding.
      setPassphraseDraft('');
      setEnableE2eDraft(false);
    } catch (err) {
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyAdopt(false);
    }
  }, [
    announce,
    buildConfig,
    deviceNameDraft,
    enableE2eDraft,
    messageForError,
    passphraseDraft,
    t,
  ]);

  const onCompact = useCallback(async () => {
    setBusyCompact(true);
    try {
      const report = await compactNow();
      announce(
        t('dialogs.settings.sync.compactDone', {
          deleted: report.deleted_logs,
        }),
      );
    } catch (err) {
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyCompact(false);
    }
  }, [announce, messageForError, t]);

  const lastSyncedLabel = (() => {
    if (!status?.last_synced_at) return t('dialogs.settings.sync.stateNeverSynced');
    try {
      const dt = new Date(status.last_synced_at);
      return t('dialogs.settings.sync.stateLastSynced', {
        time: fmt.format(dt, 'PPpp'),
      });
    } catch {
      return status.last_synced_at;
    }
  })();

  // When the adapter becomes (un)configured externally — via a
  // second app instance, or the onboarding sub-flow above — keep
  // the preview clean. A configured adapter doesn't need an
  // onboarding card.
  useEffect(() => {
    if (status?.configured) {
      setPreview(null);
    }
  }, [status?.configured]);

  return (
    <div className="sync-panel">
      <section aria-labelledby={stateHeadingId}>
        <h3 id={stateHeadingId}>{t('dialogs.settings.sync.stateTitle')}</h3>
        <p>
          {status?.configured
            ? t('dialogs.settings.sync.stateConfigured', { path: pathDraft || '–' })
            : t('dialogs.settings.sync.stateUnconfigured')}
        </p>
        {status?.configured && (
          <p>
            {status.e2e_enabled
              ? t('dialogs.settings.sync.e2eActive')
              : t('dialogs.settings.sync.e2eInactive')}
          </p>
        )}
        <p>{lastSyncedLabel}</p>
        {lastReport && (
          <p>
            {t('dialogs.settings.sync.lastReport', {
              pushed: lastReport.pushed_logs,
              fetched: lastReport.fetched_logs,
              applied: lastReport.applied,
            })}
          </p>
        )}
        {lastError && (
          <p className="sync-panel__error" role="alert">
            {t('dialogs.settings.sync.errorPrefix')}: {lastError}
          </p>
        )}
        <div className="sync-panel__actions">
          <button
            type="button"
            disabled={!status?.configured || triggering || status?.in_flight}
            onClick={() => void triggerSync()}
          >
            {status?.in_flight || triggering
              ? t('dialogs.settings.sync.syncing')
              : t('dialogs.settings.sync.syncNow')}
          </button>
          <button
            type="button"
            disabled={!status?.configured || busyCompact}
            onClick={() => void onCompact()}
          >
            {busyCompact
              ? t('dialogs.settings.sync.compacting')
              : t('dialogs.settings.sync.compactNow')}
          </button>
          {conflictCount > 0 && (
            <button type="button" onClick={openSyncConflicts}>
              {conflictCount === 1
                ? t('syncStatus.conflict_one')
                : t('syncStatus.conflict_other', { count: conflictCount })}
            </button>
          )}
        </div>
      </section>

      <section aria-labelledby={intervalHeadingId}>
        <h3 id={intervalHeadingId}>
          {t('dialogs.settings.sync.intervalLabel')}
        </h3>
        <select
          value={interval}
          onChange={(e) => void onIntervalChange(e.target.value)}
          disabled={!status?.configured}
        >
          {INTERVAL_PRESETS.map((min) => (
            <option key={min} value={min}>
              {min === 1
                ? t('dialogs.settings.sync.intervalOption_one', { minutes: min })
                : t('dialogs.settings.sync.intervalOption_other', { minutes: min })}
            </option>
          ))}
        </select>
      </section>

      <section aria-labelledby={adapterHeadingId}>
        <h3 id={adapterHeadingId}>{t('dialogs.settings.sync.adapterTitle')}</h3>
        <p>{t('dialogs.settings.sync.adapterBody')}</p>
        <div className="sync-panel__field">
          <label>
            {t('dialogs.settings.sync.adapterKind')}
            <select
              value={kindDraft}
              onChange={(e) =>
                setKindDraft(
                  e.target.value as 'local' | 'webdav' | 'sftp' | 'none',
                )
              }
            >
              <option value="local">
                {t('dialogs.settings.sync.adapterKindLocal')}
              </option>
              <option value="webdav">
                {t('dialogs.settings.sync.adapterKindWebdav')}
              </option>
              <option value="sftp">
                {t('dialogs.settings.sync.adapterKindSftp')}
              </option>
              <option value="none">
                {t('dialogs.settings.sync.adapterKindNone')}
              </option>
            </select>
          </label>
        </div>
        {kindDraft === 'local' && (
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterPath')}
              <input
                type="text"
                value={pathDraft}
                onChange={(e) => setPathDraft(e.target.value)}
                placeholder="/Volumes/NAS/aperio"
              />
            </label>
            <p className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterPathHint')}
            </p>
          </div>
        )}
        {kindDraft === 'webdav' && (
          <>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterWebdavUrl')}
                <input
                  type="url"
                  value={urlDraft}
                  onChange={(e) => setUrlDraft(e.target.value)}
                  placeholder="https://cloud.example.com/remote.php/dav/files/alice/aperio/"
                />
              </label>
              <p className="sync-panel__hint">
                {t('dialogs.settings.sync.adapterWebdavUrlHint')}
              </p>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterWebdavUser')}
                <input
                  type="text"
                  value={userDraft}
                  onChange={(e) => setUserDraft(e.target.value)}
                  autoComplete="username"
                />
              </label>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterWebdavPassword')}
                <input
                  type="password"
                  value={passwordDraft}
                  onChange={(e) => setPasswordDraft(e.target.value)}
                  autoComplete="new-password"
                  placeholder={
                    status?.configured
                      ? t('dialogs.settings.sync.adapterWebdavPasswordKept')
                      : undefined
                  }
                />
              </label>
              <p className="sync-panel__hint">
                {t('dialogs.settings.sync.adapterWebdavPasswordHint')}
              </p>
            </div>
          </>
        )}
        {kindDraft === 'sftp' && (
          <>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterSftpHost')}
                <input
                  type="text"
                  value={sftpHostDraft}
                  onChange={(e) => setSftpHostDraft(e.target.value)}
                  placeholder="nas.example.com"
                />
              </label>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterSftpPort')}
                <input
                  type="number"
                  value={sftpPortDraft}
                  onChange={(e) => setSftpPortDraft(e.target.value)}
                  min={1}
                  max={65535}
                />
              </label>
              <p className="sync-panel__hint">
                {t('dialogs.settings.sync.adapterSftpPortHint')}
              </p>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterSftpUser')}
                <input
                  type="text"
                  value={sftpUserDraft}
                  onChange={(e) => setSftpUserDraft(e.target.value)}
                  autoComplete="username"
                />
              </label>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterSftpPath')}
                <input
                  type="text"
                  value={sftpPathDraft}
                  onChange={(e) => setSftpPathDraft(e.target.value)}
                  placeholder="/home/alice/aperio"
                />
              </label>
              <p className="sync-panel__hint">
                {t('dialogs.settings.sync.adapterSftpPathHint')}
              </p>
            </div>
            <fieldset className="sync-panel__field sync-panel__authmethod">
              <legend>
                {t('dialogs.settings.sync.adapterSftpAuthMethod')}
              </legend>
              <label>
                <input
                  type="radio"
                  name="sftp-auth"
                  value="password"
                  checked={sftpAuthDraft === 'password'}
                  onChange={() => setSftpAuthDraft('password')}
                />{' '}
                {t('dialogs.settings.sync.adapterSftpAuthPassword')}
              </label>
              <label>
                <input
                  type="radio"
                  name="sftp-auth"
                  value="key"
                  checked={sftpAuthDraft === 'key'}
                  onChange={() => setSftpAuthDraft('key')}
                />{' '}
                {t('dialogs.settings.sync.adapterSftpAuthKey')}
              </label>
            </fieldset>
            {sftpAuthDraft === 'password' && (
              <div className="sync-panel__field">
                <label>
                  {t('dialogs.settings.sync.adapterSftpPassword')}
                  <input
                    type="password"
                    value={sftpPasswordDraft}
                    onChange={(e) =>
                      setSftpPasswordDraft(e.target.value)
                    }
                    autoComplete="new-password"
                    placeholder={
                      status?.configured
                        ? t(
                            'dialogs.settings.sync.adapterWebdavPasswordKept',
                          )
                        : undefined
                    }
                  />
                </label>
                <p className="sync-panel__hint">
                  {t('dialogs.settings.sync.adapterSftpPasswordHint')}
                </p>
              </div>
            )}
            {sftpAuthDraft === 'key' && (
              <>
                <div className="sync-panel__field">
                  <label>
                    {t('dialogs.settings.sync.adapterSftpKeyPath')}
                    <input
                      type="text"
                      value={sftpKeyPathDraft}
                      onChange={(e) =>
                        setSftpKeyPathDraft(e.target.value)
                      }
                      placeholder="/home/alice/.ssh/id_ed25519"
                    />
                  </label>
                  <p className="sync-panel__hint">
                    {t('dialogs.settings.sync.adapterSftpKeyPathHint')}
                  </p>
                </div>
                <div className="sync-panel__field">
                  <label>
                    {t('dialogs.settings.sync.adapterSftpKeyPassphrase')}
                    <input
                      type="password"
                      value={sftpKeyPassphraseDraft}
                      onChange={(e) =>
                        setSftpKeyPassphraseDraft(e.target.value)
                      }
                      autoComplete="new-password"
                      placeholder={
                        status?.configured
                          ? t(
                              'dialogs.settings.sync.adapterWebdavPasswordKept',
                            )
                          : undefined
                      }
                    />
                  </label>
                  <p className="sync-panel__hint">
                    {t(
                      'dialogs.settings.sync.adapterSftpKeyPassphraseHint',
                    )}
                  </p>
                </div>
              </>
            )}
          </>
        )}
        <div className="sync-panel__actions">
          <button
            type="button"
            disabled={busyAdapter}
            onClick={() => void onConfigure()}
          >
            {busyAdapter
              ? t('dialogs.settings.sync.adapterConnecting')
              : t('dialogs.settings.sync.adapterConfigure')}
          </button>
          {status?.configured && (
            <button
              type="button"
              disabled={busyAdapter}
              onClick={() => void onDisconnect()}
            >
              {t('dialogs.settings.sync.adapterDisconnect')}
            </button>
          )}
        </div>
      </section>

      {!status?.configured && (
        <section aria-labelledby={previewHeadingId}>
          <h3 id={previewHeadingId}>
            {t('dialogs.settings.sync.previewTitle')}
          </h3>
          <p>{t('dialogs.settings.sync.previewBody')}</p>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.deviceName')}
              <input
                type="text"
                value={deviceNameDraft}
                onChange={(e) => setDeviceNameDraft(e.target.value)}
                placeholder="Desktop"
              />
            </label>
            <p className="sync-panel__hint">
              {t('dialogs.settings.sync.deviceNameHint')}
            </p>
          </div>
          <button
            type="button"
            disabled={busyPreview}
            onClick={() => void onPreview()}
          >
            {t('dialogs.settings.sync.previewButton')}
          </button>
          {previewError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {previewError}
            </p>
          )}
          {preview?.kind === 'empty' && (
            <>
              <E2eEnableInput
                enabled={enableE2eDraft}
                onToggle={setEnableE2eDraft}
                passphrase={passphraseDraft}
                onPassphraseChange={setPassphraseDraft}
                t={t}
              />
              <PreviewEmpty
                t={t}
                busyAdopt={busyAdopt}
                onAdopt={onAdopt}
              />
            </>
          )}
          {preview?.kind === 'existing' && (
            <>
              {preview.e2e_enabled && (
                <E2ePassphrasePrompt
                  passphrase={passphraseDraft}
                  onPassphraseChange={setPassphraseDraft}
                  t={t}
                />
              )}
              <PreviewExisting
                preview={preview}
                t={t}
                fmt={fmt}
                busyAccept={busyAccept}
                busyAdopt={busyAdopt}
                onAccept={onAccept}
                onAdopt={onAdopt}
              />
            </>
          )}
        </section>
      )}
    </div>
  );
}

function PreviewEmpty({
  t,
  busyAdopt,
  onAdopt,
}: {
  t: ReturnType<typeof useTranslation>['t'];
  busyAdopt: boolean;
  onAdopt: () => void;
}) {
  return (
    <div className="sync-panel__preview">
      <p>{t('dialogs.settings.sync.previewEmpty')}</p>
      <button
        type="button"
        disabled={busyAdopt}
        onClick={onAdopt}
      >
        {t('dialogs.settings.sync.previewAdoptButton')}
      </button>
    </div>
  );
}

function PreviewExisting({
  preview,
  t,
  fmt,
  busyAccept,
  busyAdopt,
  onAccept,
  onAdopt,
}: {
  preview: Extract<SyncPreview, { kind: 'existing' }>;
  t: ReturnType<typeof useTranslation>['t'];
  fmt: ReturnType<typeof useDateFormat>;
  busyAccept: boolean;
  busyAdopt: boolean;
  onAccept: () => void;
  onAdopt: () => void;
}) {
  const summary =
    preview.snapshot_timestamp !== null
      ? t('dialogs.settings.sync.previewExisting', {
          time: (() => {
            try {
              return fmt.format(
                new Date(preview.snapshot_timestamp as string),
                'PP',
              );
            } catch {
              return preview.snapshot_timestamp;
            }
          })(),
        })
      : t('dialogs.settings.sync.previewNeverCompacted');
  const names = preview.devices
    .map((d) =>
      d.is_this_device
        ? `${d.name ?? d.id} (${t('dialogs.settings.sync.previewThisDevice')})`
        : (d.name ?? d.id),
    )
    .join(', ');
  return (
    <div className="sync-panel__preview">
      <p>{summary}</p>
      <p>
        {t('dialogs.settings.sync.previewDevices', {
          count: preview.devices.length,
          names,
        })}
      </p>
      <div className="sync-panel__preview-actions">
        <div>
          <h4>{t('dialogs.settings.sync.previewAcceptTitle')}</h4>
          <p>{t('dialogs.settings.sync.previewAcceptBody')}</p>
          <button
            type="button"
            disabled={busyAccept}
            onClick={onAccept}
          >
            {t('dialogs.settings.sync.previewAcceptButton')}
          </button>
        </div>
        <div>
          <h4>{t('dialogs.settings.sync.previewAdoptTitle')}</h4>
          <p>{t('dialogs.settings.sync.previewAdoptBody')}</p>
          <button
            type="button"
            disabled={busyAdopt}
            onClick={onAdopt}
          >
            {t('dialogs.settings.sync.previewAdoptButton')}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Sub-form rendered above the "Start fresh" button when the
 *  remote is empty. Checkbox + passphrase input that wire to the
 *  adopt_local flow's optional `passphrase` argument. The
 *  passphrase is intentionally NOT confirmed twice — §19.7 makes
 *  the irreversibility clear in the surrounding copy and the user
 *  is the only person who can ever recover the dataset anyway. */
function E2eEnableInput({
  enabled,
  onToggle,
  passphrase,
  onPassphraseChange,
  t,
}: {
  enabled: boolean;
  onToggle: (next: boolean) => void;
  passphrase: string;
  onPassphraseChange: (next: string) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="sync-panel__e2e">
      <label className="sync-panel__field">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => onToggle(e.target.checked)}
        />{' '}
        {t('dialogs.settings.sync.e2eEnableLabel')}
      </label>
      <p className="sync-panel__hint">
        {t('dialogs.settings.sync.e2eEnableHint')}
      </p>
      {enabled && (
        <div className="sync-panel__field">
          <label>
            {t('dialogs.settings.sync.e2ePassphrase')}
            <input
              type="password"
              value={passphrase}
              onChange={(e) => onPassphraseChange(e.target.value)}
              autoComplete="new-password"
            />
          </label>
          <p className="sync-panel__hint sync-panel__hint--warning">
            {t('dialogs.settings.sync.e2eIrreversibleWarning')}
          </p>
        </div>
      )}
    </div>
  );
}

/** Sub-form rendered above the accept/adopt cards when the
 *  preview reports `e2e_enabled = true`. The user MUST type the
 *  dataset's passphrase to derive the key and decrypt anything
 *  during onboarding. */
function E2ePassphrasePrompt({
  passphrase,
  onPassphraseChange,
  t,
}: {
  passphrase: string;
  onPassphraseChange: (next: string) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="sync-panel__e2e">
      <p>{t('dialogs.settings.sync.e2eRemoteRequiresPassphrase')}</p>
      <div className="sync-panel__field">
        <label>
          {t('dialogs.settings.sync.e2ePassphrase')}
          <input
            type="password"
            value={passphrase}
            onChange={(e) => onPassphraseChange(e.target.value)}
            autoComplete="current-password"
          />
        </label>
      </div>
    </div>
  );
}
