import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

import { useAnnouncer } from '../../a11y/announcerContext';
import { FocusableNote } from '../../a11y/FocusableNote';
import {
  acceptRemoteDataset,
  adoptLocalDataset,
  connectDropboxOauth,
  connectGoogledriveOauth,
  forgetSftpHostKey,
  getPinnedSftpHostKey,
  hasDropboxRefreshToken,
  hasGoogledriveRefreshToken,
  previewSftpHostKey,
  previewSyncTarget,
  refreshExternalCache,
  testSyncAdapter,
  trustSftpHostKey,
  type HostKeyPreview,
  type SyncAdapterConfig,
  type SyncPreview,
  type SyncStatus,
} from '../../api/client';
import type { Account } from '../../api/types';
import { useDateFormat } from '../../intl/dateFormat';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useDialogState } from '../../state/dialogStateContext';
import { fetchAccountsNeedingConnect } from '../accountsNeedingConnect';
import { SyncSftpTrustDialog } from '../SyncSftpTrustDialog';
import { useSyncErrorMessage } from './syncErrorMessage';

/** What a successful connect produced — handed to the caller so it can decide
 *  what to do next (refresh its summary card, advance a wizard, prompt for
 *  missing account credentials, …). */
export interface SyncConnectOutcome {
  /** `true` when we JOINED an existing remote dataset (restore), `false` when
   *  we initialized a fresh one on an empty target (create). */
  joined: boolean;
  /** Accounts whose secrets didn't arrive with a restored dataset and need the
   *  user to re-enter credentials. Empty for a freshly-created dataset. */
  accountsNeedingConnect: Account[];
}

export interface SyncTargetConfigFormProps {
  /** Current sync status — drives placeholder/disabled hints. */
  status: SyncStatus | null | undefined;
  /** Called after a connect succeeds. The form has already refreshed the local
   *  catalogs/data and cleared its drafts; the caller owns whatever comes next. */
  onConnected: (outcome: SyncConnectOutcome) => void;
}

type AdapterKindDraft =
  | 'local'
  | 'webdav'
  | 'sftp'
  | 'ftp'
  | 'dropbox'
  | 'googledrive'
  | 'none';

/**
 * The sync-target CONFIGURATION form (DESIGN.md §19.11) — adapter-kind picker,
 * per-kind fields (local / WebDAV / SFTP / FTPS / Dropbox / Google Drive),
 * OAuth sign-in, SFTP host-key TOFU trust, the device name, and the unified
 * "Verbinden" that previews the target then joins it (restore) or initializes
 * it (create), with optional E2E.
 *
 * Extracted from `SyncPanel` so the Settings → Sync panel AND the first-launch
 * wizard share ONE implementation. The component owns all of its own state and
 * side effects; the host only supplies `status` and an `onConnected` callback.
 */
export function SyncTargetConfigForm({
  status,
  onConnected,
}: SyncTargetConfigFormProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const { invalidateData } = useDialogState();
  // After onboarding pulls a whole dataset into the local DB, the sidebar's
  // container catalogs don't re-read on a dataVersion bump — refresh them
  // explicitly so the restored local containers don't stay hidden until a
  // restart.
  const {
    refreshCalendars,
    refreshTaskLists,
    refreshContactLists,
    refreshColorLabels,
    refreshAccounts,
  } = useCalendarStore();
  const refreshCatalogs = useCallback(() => {
    void refreshCalendars();
    void refreshTaskLists();
    void refreshContactLists();
    void refreshColorLabels();
    void refreshAccounts();
  }, [
    refreshAccounts,
    refreshCalendars,
    refreshColorLabels,
    refreshContactLists,
    refreshTaskLists,
  ]);
  const messageForError = useSyncErrorMessage();

  // Adapter draft state. Seeded to sensible defaults; the persisted password /
  // key material lives in the OS keychain and is never surfaced back here.
  const [kindDraft, setKindDraft] = useState<AdapterKindDraft>('local');
  const [pathDraft, setPathDraft] = useState('');
  // WebDAV.
  const [urlDraft, setUrlDraft] = useState('');
  const [userDraft, setUserDraft] = useState('');
  const [passwordDraft, setPasswordDraft] = useState('');
  // SFTP. Port is a string so the input doesn't fight React on every keystroke.
  const [sftpHostDraft, setSftpHostDraft] = useState('');
  const [sftpPortDraft, setSftpPortDraft] = useState('22');
  const [sftpUserDraft, setSftpUserDraft] = useState('');
  const [sftpPathDraft, setSftpPathDraft] = useState('');
  const [sftpPasswordDraft, setSftpPasswordDraft] = useState('');
  const [sftpAuthDraft, setSftpAuthDraft] = useState<'password' | 'key'>(
    'password',
  );
  const [sftpKeyPathDraft, setSftpKeyPathDraft] = useState('');
  const [sftpKeyPassphraseDraft, setSftpKeyPassphraseDraft] = useState('');
  // FTPS. Port default flips with the mode (21 explicit/plain, 990 implicit).
  const [ftpHostDraft, setFtpHostDraft] = useState('');
  const [ftpPortDraft, setFtpPortDraft] = useState('21');
  const [ftpUserDraft, setFtpUserDraft] = useState('');
  const [ftpPathDraft, setFtpPathDraft] = useState('');
  const [ftpPasswordDraft, setFtpPasswordDraft] = useState('');
  const [ftpModeDraft, setFtpModeDraft] = useState<
    'explicit' | 'implicit' | 'plain'
  >('explicit');
  // Dropbox.
  const [dropboxClientIdDraft, setDropboxClientIdDraft] = useState('');
  const [dropboxClientSecretDraft, setDropboxClientSecretDraft] = useState('');
  const [dropboxPathDraft, setDropboxPathDraft] = useState('');
  const [busyDropboxOauth, setBusyDropboxOauth] = useState(false);
  const [dropboxSignedIn, setDropboxSignedIn] = useState(false);
  // Google Drive.
  const [gdriveClientIdDraft, setGdriveClientIdDraft] = useState('');
  const [gdriveClientSecretDraft, setGdriveClientSecretDraft] = useState('');
  const [gdriveFolderNameDraft, setGdriveFolderNameDraft] = useState('');
  const [busyGdriveOauth, setBusyGdriveOauth] = useState(false);
  const [gdriveSignedIn, setGdriveSignedIn] = useState(false);
  // E2E onboarding inputs. `passphraseDraft` doubles as the new-dataset
  // passphrase (adopt_local) and the unlock passphrase (accept_remote);
  // `enableE2eDraft` is the "encrypt the new dataset" toggle (adopt only).
  const [passphraseDraft, setPassphraseDraft] = useState('');
  const [enableE2eDraft, setEnableE2eDraft] = useState(false);
  const [deviceNameDraft, setDeviceNameDraft] = useState('');
  const [busyAdapter, setBusyAdapter] = useState(false);
  const [busyTest, setBusyTest] = useState(false);
  const [busyAdopt, setBusyAdopt] = useState(false);
  const [adapterFeedback, setAdapterFeedback] = useState<{
    kind: 'ok' | 'error';
    message: string;
  } | null>(null);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  // SFTP host-key trust dialog state.
  const [trustPreview, setTrustPreview] = useState<HostKeyPreview | null>(null);
  const [pendingSftpConfig, setPendingSftpConfig] =
    useState<SyncAdapterConfig | null>(null);
  const [pinnedFingerprint, setPinnedFingerprint] = useState<string | null>(
    null,
  );

  const buildConfig = useCallback((): SyncAdapterConfig => {
    if (kindDraft === 'local') return { kind: 'local', path: pathDraft.trim() };
    if (kindDraft === 'webdav') {
      return {
        kind: 'webdav',
        url: urlDraft.trim(),
        user: userDraft.trim(),
        // Empty string → backend reuses the keychain entry.
        password: passwordDraft.trim() || null,
      };
    }
    if (kindDraft === 'sftp') {
      const port = Number.parseInt(sftpPortDraft, 10);
      return {
        kind: 'sftp',
        host: sftpHostDraft.trim(),
        port: Number.isFinite(port) && port > 0 ? port : 22,
        user: sftpUserDraft.trim(),
        path: sftpPathDraft.trim(),
        auth_method: sftpAuthDraft,
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
    if (kindDraft === 'dropbox') {
      return {
        kind: 'dropbox',
        client_id: dropboxClientIdDraft.trim(),
        client_secret: dropboxClientSecretDraft.trim(),
        path: dropboxPathDraft.trim(),
      };
    }
    if (kindDraft === 'googledrive') {
      return {
        kind: 'googledrive',
        client_id: gdriveClientIdDraft.trim(),
        client_secret: gdriveClientSecretDraft.trim(),
        folder_name: gdriveFolderNameDraft.trim(),
      };
    }
    if (kindDraft === 'ftp') {
      const port = Number.parseInt(ftpPortDraft, 10);
      const fallback = ftpModeDraft === 'implicit' ? 990 : 21;
      return {
        kind: 'ftp',
        host: ftpHostDraft.trim(),
        port: Number.isFinite(port) && port > 0 ? port : fallback,
        user: ftpUserDraft.trim(),
        path: ftpPathDraft.trim(),
        mode: ftpModeDraft,
        password: ftpPasswordDraft.trim() || null,
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
    ftpHostDraft,
    ftpPortDraft,
    ftpUserDraft,
    ftpPathDraft,
    ftpPasswordDraft,
    ftpModeDraft,
    dropboxClientIdDraft,
    dropboxClientSecretDraft,
    dropboxPathDraft,
    gdriveClientIdDraft,
    gdriveClientSecretDraft,
    gdriveFolderNameDraft,
  ]);

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
      if (
        sftpAuthDraft === 'key' &&
        !sftpKeyPathDraft.trim() &&
        !status?.configured
      ) {
        return true;
      }
      return false;
    }
    if (kindDraft === 'ftp') {
      return !ftpHostDraft.trim() || !ftpUserDraft.trim();
    }
    if (kindDraft === 'dropbox') {
      return !dropboxClientIdDraft.trim() || !dropboxSignedIn;
    }
    if (kindDraft === 'googledrive') {
      return (
        !gdriveClientIdDraft.trim() ||
        !gdriveClientSecretDraft.trim() ||
        !gdriveSignedIn
      );
    }
    return false;
  })();

  // Unified "Verbinden": inspect the target and do the right thing in one
  // gesture — join an existing dataset (restore) or, on an empty target,
  // reveal the optional encryption setup then initialize it (create).
  const runConnect = useCallback(
    async (config: SyncAdapterConfig) => {
      setBusyAdapter(true);
      setAdapterFeedback(null);
      const priorPreview = preview;
      try {
        const p = await previewSyncTarget(config);
        setPreview(p);
        const device = deviceNameDraft.trim() || null;
        let joined = false;
        let accountsNeedingConnect: Account[] = [];

        if (p.kind === 'existing') {
          if (p.e2e_enabled && !passphraseDraft.trim()) {
            setAdapterFeedback({
              kind: 'error',
              message: t('dialogs.settings.sync.e2eRemoteRequiresPassphrase'),
            });
            return;
          }
          const report = await acceptRemoteDataset(
            config,
            device,
            p.e2e_enabled ? passphraseDraft.trim() : null,
          );
          joined = true;
          announce(
            report.device_count === 1
              ? t('dialogs.settings.sync.onboardingDone_one')
              : t('dialogs.settings.sync.onboardingDone_other', {
                  count: report.device_count,
                }),
          );
          // §19.11 step 8: surface accounts whose secrets didn't arrive.
          accountsNeedingConnect = (await fetchAccountsNeedingConnect()) ?? [];
        } else {
          // Empty target → initialize from this device. On first surface,
          // reveal the optional encryption setup and require a second click.
          if (priorPreview?.kind !== 'empty') {
            setAdapterFeedback({
              kind: 'ok',
              message: t('dialogs.settings.sync.connectEmptyReveal'),
            });
            return;
          }
          if (enableE2eDraft && !passphraseDraft.trim()) {
            setAdapterFeedback({
              kind: 'error',
              message: t('dialogs.settings.sync.e2ePassphraseRequired'),
            });
            return;
          }
          await adoptLocalDataset(
            config,
            device,
            enableE2eDraft ? passphraseDraft.trim() : null,
          );
          announce(t('dialogs.settings.sync.onboardingFresh'));
        }

        // Shared success bookkeeping.
        if (config.kind === 'webdav') setPasswordDraft('');
        if (config.kind === 'sftp') {
          setSftpPasswordDraft('');
          setSftpKeyPassphraseDraft('');
        }
        setPassphraseDraft('');
        setEnableE2eDraft(false);
        setPreview(null);
        // Make the just-onboarded data visible WITHOUT a restart. We do NOT
        // trigger a sync round here — the backend already kicks the scheduler
        // at the end of accept/adopt.
        refreshCatalogs();
        invalidateData();
        void refreshExternalCache();
        onConnected({ joined, accountsNeedingConnect });
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('connect failed', err);
        setAdapterFeedback({
          kind: 'error',
          message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        });
        setPreview(null);
      } finally {
        setBusyAdapter(false);
      }
    },
    [
      announce,
      deviceNameDraft,
      enableE2eDraft,
      invalidateData,
      messageForError,
      onConnected,
      passphraseDraft,
      preview,
      refreshCatalogs,
      t,
    ],
  );

  const onConnect = useCallback(async () => {
    setAdapterFeedback(null);
    if (configMissingRequired) {
      setAdapterFeedback({
        kind: 'error',
        message: t('dialogs.settings.sync.adapterNeedPath'),
      });
      return;
    }
    const config = buildConfig();
    // SFTP: probe the host key BEFORE connecting (§19.5 trust gesture).
    if (config.kind === 'sftp') {
      setBusyAdapter(true);
      let previewResult: HostKeyPreview;
      try {
        previewResult = await previewSftpHostKey(config.host, config.port);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('preview_sftp_host_key failed', err);
        setAdapterFeedback({
          kind: 'error',
          message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        });
        setBusyAdapter(false);
        return;
      }
      if (previewResult.status.kind === 'unchanged') {
        setBusyAdapter(false);
        await runConnect(config);
        return;
      }
      // Hand off to the trust dialog; park the config for the resume.
      setBusyAdapter(false);
      setPendingSftpConfig(config);
      setTrustPreview(previewResult);
      return;
    }
    await runConnect(config);
  }, [buildConfig, configMissingRequired, messageForError, runConnect, t]);

  const onTrustAccept = useCallback(
    async (fingerprint: string) => {
      const config = pendingSftpConfig;
      const trusted = trustPreview;
      setTrustPreview(null);
      setPendingSftpConfig(null);
      if (!config || !trusted) return;
      try {
        await trustSftpHostKey(trusted.host_port, fingerprint);
        setPinnedFingerprint(fingerprint);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('trust_sftp_host_key failed', err);
        setAdapterFeedback({
          kind: 'error',
          message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        });
        return;
      }
      await runConnect(config);
    },
    [messageForError, pendingSftpConfig, runConnect, t, trustPreview],
  );

  const onTrustCancel = useCallback(() => {
    setTrustPreview(null);
    setPendingSftpConfig(null);
  }, []);

  const onTest = useCallback(async () => {
    setAdapterFeedback(null);
    if (configMissingRequired) {
      setAdapterFeedback({
        kind: 'error',
        message: t('dialogs.settings.sync.adapterNeedPath'),
      });
      return;
    }
    setBusyTest(true);
    try {
      await testSyncAdapter(buildConfig());
      setAdapterFeedback({
        kind: 'ok',
        message: t('dialogs.settings.sync.adapterTestOk'),
      });
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('test_sync_adapter failed', err);
      setAdapterFeedback({
        kind: 'error',
        message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
      });
    } finally {
      setBusyTest(false);
    }
  }, [buildConfig, configMissingRequired, messageForError, t]);

  // Destructive secondary action for an EXISTING dataset: discard it and
  // re-initialize from this device (plaintext; encryption can be enabled
  // afterwards). Behind an explicit confirm — the default "Verbinden" joins.
  const onOverwrite = useCallback(async () => {
    if (!window.confirm(t('dialogs.settings.sync.previewAdoptConfirm'))) return;
    setBusyAdopt(true);
    setAdapterFeedback(null);
    try {
      await adoptLocalDataset(buildConfig(), deviceNameDraft.trim() || null, null);
      announce(t('dialogs.settings.sync.onboardingFresh'));
      setPreview(null);
      setPassphraseDraft('');
      setEnableE2eDraft(false);
      refreshCatalogs();
      invalidateData();
      void refreshExternalCache();
      onConnected({ joined: false, accountsNeedingConnect: [] });
    } catch (err) {
      setAdapterFeedback({
        kind: 'error',
        message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
      });
    } finally {
      setBusyAdopt(false);
    }
  }, [
    announce,
    buildConfig,
    deviceNameDraft,
    invalidateData,
    messageForError,
    onConnected,
    refreshCatalogs,
    t,
  ]);

  const onBrowseLocalPath = useCallback(async () => {
    try {
      const selected = await openFileDialog({
        multiple: false,
        directory: true,
        title: t('dialogs.settings.sync.adapterPathDialogTitle'),
        defaultPath: pathDraft.trim() || undefined,
      });
      if (typeof selected === 'string' && selected.length > 0) {
        setPathDraft(selected);
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('local path picker failed', err);
    }
  }, [pathDraft, t]);

  const onBrowseKey = useCallback(async () => {
    try {
      const selected = await openFileDialog({
        multiple: false,
        directory: false,
        title: t('dialogs.settings.sync.adapterSftpKeyPathDialogTitle'),
      });
      if (typeof selected === 'string' && selected.length > 0) {
        setSftpKeyPathDraft(selected);
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('SSH key picker failed', err);
    }
  }, [t]);

  // ── SFTP pinned-fingerprint readout ──
  useEffect(() => {
    if (kindDraft !== 'sftp') {
      setPinnedFingerprint(null);
      return;
    }
    const host = sftpHostDraft.trim();
    if (!host) {
      setPinnedFingerprint(null);
      return;
    }
    const port = Number.parseInt(sftpPortDraft, 10);
    const resolvedPort = Number.isFinite(port) && port > 0 ? port : 22;
    const hostPort = `${host}:${resolvedPort}`;
    let cancelled = false;
    getPinnedSftpHostKey(hostPort)
      .then((fp) => {
        if (!cancelled) setPinnedFingerprint(fp);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('get_pinned_sftp_host_key failed', err);
        if (!cancelled) setPinnedFingerprint(null);
      });
    return () => {
      cancelled = true;
    };
  }, [kindDraft, sftpHostDraft, sftpPortDraft]);

  const onForgetPin = useCallback(async () => {
    const host = sftpHostDraft.trim();
    if (!host) return;
    const port = Number.parseInt(sftpPortDraft, 10);
    const resolvedPort = Number.isFinite(port) && port > 0 ? port : 22;
    const hostPort = `${host}:${resolvedPort}`;
    const confirmed = window.confirm(
      t('dialogs.settings.sync.sftpForgetPinConfirm', { hostPort }),
    );
    if (!confirmed) return;
    try {
      await forgetSftpHostKey(hostPort);
      setPinnedFingerprint(null);
      announce(t('dialogs.settings.sync.sftpForgetPinDone'));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('forget_sftp_host_key failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    }
  }, [announce, messageForError, sftpHostDraft, sftpPortDraft, t]);

  // ── OAuth (Dropbox / Google Drive) ──
  const refreshDropboxSignedIn = useCallback(() => {
    hasDropboxRefreshToken()
      .then(setDropboxSignedIn)
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('has_dropbox_refresh_token failed', err);
        setDropboxSignedIn(false);
      });
  }, []);
  useEffect(() => {
    refreshDropboxSignedIn();
  }, [refreshDropboxSignedIn]);

  const onConnectDropbox = useCallback(async () => {
    setAdapterFeedback(null);
    const clientId = dropboxClientIdDraft.trim();
    if (!clientId) {
      setAdapterFeedback({
        kind: 'error',
        message: t('dialogs.settings.sync.adapterDropboxNeedsClientId'),
      });
      return;
    }
    setBusyDropboxOauth(true);
    try {
      await connectDropboxOauth(clientId, dropboxClientSecretDraft.trim());
      announce(t('dialogs.settings.sync.adapterDropboxSignedInAnnouncement'));
      refreshDropboxSignedIn();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('connect_dropbox_oauth failed', err);
      setAdapterFeedback({
        kind: 'error',
        message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
      });
    } finally {
      setBusyDropboxOauth(false);
    }
  }, [
    announce,
    dropboxClientIdDraft,
    dropboxClientSecretDraft,
    messageForError,
    refreshDropboxSignedIn,
    t,
  ]);

  const refreshGdriveSignedIn = useCallback(() => {
    hasGoogledriveRefreshToken()
      .then(setGdriveSignedIn)
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('has_googledrive_refresh_token failed', err);
        setGdriveSignedIn(false);
      });
  }, []);
  useEffect(() => {
    refreshGdriveSignedIn();
  }, [refreshGdriveSignedIn]);

  const onConnectGoogledrive = useCallback(async () => {
    setAdapterFeedback(null);
    const clientId = gdriveClientIdDraft.trim();
    const clientSecret = gdriveClientSecretDraft.trim();
    if (!clientId || !clientSecret) {
      setAdapterFeedback({
        kind: 'error',
        message: t('dialogs.settings.sync.adapterGoogledriveNeedsClientId'),
      });
      return;
    }
    setBusyGdriveOauth(true);
    try {
      await connectGoogledriveOauth(clientId, clientSecret);
      announce(
        t('dialogs.settings.sync.adapterGoogledriveSignedInAnnouncement'),
      );
      refreshGdriveSignedIn();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('connect_googledrive_oauth failed', err);
      setAdapterFeedback({
        kind: 'error',
        message: `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
      });
    } finally {
      setBusyGdriveOauth(false);
    }
  }, [
    announce,
    gdriveClientIdDraft,
    gdriveClientSecretDraft,
    messageForError,
    refreshGdriveSignedIn,
    t,
  ]);

  return (
    <>
      <FocusableNote className="sync-panel__hint">
        {t('dialogs.settings.sync.adapterBody')}
      </FocusableNote>
      <div className="sync-panel__field">
        <label>
          {t('dialogs.settings.sync.adapterKind')}
          <select
            value={kindDraft}
            onChange={(e) => setKindDraft(e.target.value as AdapterKindDraft)}
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
            <option value="ftp">
              {t('dialogs.settings.sync.adapterKindFtp')}
            </option>
            <option value="dropbox">
              {t('dialogs.settings.sync.adapterKindDropbox')}
            </option>
            <option value="googledrive">
              {t('dialogs.settings.sync.adapterKindGoogledrive')}
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
            <div className="sync-panel__filepicker">
              <input
                type="text"
                value={pathDraft}
                onChange={(e) => setPathDraft(e.target.value)}
                placeholder="/Volumes/NAS/aperio"
              />
              <button
                type="button"
                onClick={() => void onBrowseLocalPath()}
                aria-label={t('dialogs.settings.sync.adapterPathBrowseAria')}
              >
                {t('dialogs.settings.sync.adapterPathBrowse')}
              </button>
            </div>
          </label>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.adapterPathHint')}
          </FocusableNote>
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
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterWebdavUrlHint')}
            </FocusableNote>
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
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterWebdavPasswordHint')}
            </FocusableNote>
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
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterSftpPortHint')}
            </FocusableNote>
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
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterSftpPathHint')}
            </FocusableNote>
          </div>
          <fieldset className="sync-panel__field sync-panel__authmethod">
            <legend>{t('dialogs.settings.sync.adapterSftpAuthMethod')}</legend>
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
                  onChange={(e) => setSftpPasswordDraft(e.target.value)}
                  autoComplete="new-password"
                  placeholder={
                    status?.configured
                      ? t('dialogs.settings.sync.adapterWebdavPasswordKept')
                      : undefined
                  }
                />
              </label>
              <FocusableNote className="sync-panel__hint">
                {t('dialogs.settings.sync.adapterSftpPasswordHint')}
              </FocusableNote>
            </div>
          )}
          {pinnedFingerprint && (
            <div className="sync-panel__field sync-panel__pin">
              <FocusableNote className="sync-panel__hint">
                {t('dialogs.settings.sync.sftpPinCurrentWithValue', {
                  fingerprint: pinnedFingerprint,
                })}
              </FocusableNote>
              <FocusableNote className="sync-panel__hint">
                {t('dialogs.settings.sync.sftpPinHint')}
              </FocusableNote>
              <button type="button" onClick={() => void onForgetPin()}>
                {t('dialogs.settings.sync.sftpForgetPin')}
              </button>
            </div>
          )}
          {sftpAuthDraft === 'key' && (
            <>
              <div className="sync-panel__field">
                <label>
                  {t('dialogs.settings.sync.adapterSftpKeyPath')}
                  <div className="sync-panel__filepicker">
                    <input
                      type="text"
                      value={sftpKeyPathDraft}
                      onChange={(e) => setSftpKeyPathDraft(e.target.value)}
                      placeholder="/home/alice/.ssh/id_ed25519"
                    />
                    <button
                      type="button"
                      onClick={() => void onBrowseKey()}
                      aria-label={t(
                        'dialogs.settings.sync.adapterSftpKeyPathBrowseAria',
                      )}
                    >
                      {t('dialogs.settings.sync.adapterSftpKeyPathBrowse')}
                    </button>
                  </div>
                </label>
                <FocusableNote className="sync-panel__hint">
                  {t('dialogs.settings.sync.adapterSftpKeyPathHint')}
                </FocusableNote>
              </div>
              <div className="sync-panel__field">
                <label>
                  {t('dialogs.settings.sync.adapterSftpKeyPassphrase')}
                  <input
                    type="password"
                    value={sftpKeyPassphraseDraft}
                    onChange={(e) => setSftpKeyPassphraseDraft(e.target.value)}
                    autoComplete="new-password"
                    placeholder={
                      status?.configured
                        ? t('dialogs.settings.sync.adapterWebdavPasswordKept')
                        : undefined
                    }
                  />
                </label>
                <FocusableNote className="sync-panel__hint">
                  {t('dialogs.settings.sync.adapterSftpKeyPassphraseHint')}
                </FocusableNote>
              </div>
            </>
          )}
        </>
      )}
      {kindDraft === 'ftp' && (
        <>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterFtpHost')}
              <input
                type="text"
                value={ftpHostDraft}
                onChange={(e) => setFtpHostDraft(e.target.value)}
                placeholder="ftp.example.com"
              />
            </label>
          </div>
          <fieldset className="sync-panel__field sync-panel__authmethod">
            <legend>{t('dialogs.settings.sync.adapterFtpMode')}</legend>
            <label>
              <input
                type="radio"
                name="ftp-mode"
                value="explicit"
                checked={ftpModeDraft === 'explicit'}
                onChange={() => {
                  setFtpModeDraft('explicit');
                  if (ftpPortDraft === '990') setFtpPortDraft('21');
                }}
              />{' '}
              {t('dialogs.settings.sync.adapterFtpModeExplicit')}
            </label>
            <label>
              <input
                type="radio"
                name="ftp-mode"
                value="implicit"
                checked={ftpModeDraft === 'implicit'}
                onChange={() => {
                  setFtpModeDraft('implicit');
                  if (ftpPortDraft === '21') setFtpPortDraft('990');
                }}
              />{' '}
              {t('dialogs.settings.sync.adapterFtpModeImplicit')}
            </label>
            <label>
              <input
                type="radio"
                name="ftp-mode"
                value="plain"
                checked={ftpModeDraft === 'plain'}
                onChange={() => {
                  setFtpModeDraft('plain');
                  if (ftpPortDraft === '990') setFtpPortDraft('21');
                }}
              />{' '}
              {t('dialogs.settings.sync.adapterFtpModePlain')}
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterFtpModeHint')}
            </FocusableNote>
            {ftpModeDraft === 'plain' && (
              <p className="sync-panel__warning" role="alert">
                {t('dialogs.settings.sync.adapterFtpModePlainWarning')}
              </p>
            )}
          </fieldset>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterFtpPort')}
              <input
                type="number"
                value={ftpPortDraft}
                onChange={(e) => setFtpPortDraft(e.target.value)}
                min={1}
                max={65535}
              />
            </label>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterFtpUser')}
              <input
                type="text"
                value={ftpUserDraft}
                onChange={(e) => setFtpUserDraft(e.target.value)}
                autoComplete="username"
              />
            </label>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterFtpPath')}
              <input
                type="text"
                value={ftpPathDraft}
                onChange={(e) => setFtpPathDraft(e.target.value)}
                placeholder="/aperio"
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterFtpPathHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterFtpPassword')}
              <input
                type="password"
                value={ftpPasswordDraft}
                onChange={(e) => setFtpPasswordDraft(e.target.value)}
                autoComplete="new-password"
                placeholder={
                  status?.configured
                    ? t('dialogs.settings.sync.adapterWebdavPasswordKept')
                    : undefined
                }
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {ftpModeDraft === 'plain'
                ? t('dialogs.settings.sync.adapterFtpPlainPasswordHint')
                : t('dialogs.settings.sync.adapterFtpTlsRequiredHint')}
            </FocusableNote>
          </div>
        </>
      )}
      {kindDraft === 'dropbox' && (
        <>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.adapterDropboxIntro')}
          </FocusableNote>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterDropboxClientId')}
              <input
                type="text"
                value={dropboxClientIdDraft}
                onChange={(e) => setDropboxClientIdDraft(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterDropboxClientIdHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterDropboxClientSecret')}
              <input
                type="password"
                value={dropboxClientSecretDraft}
                onChange={(e) => setDropboxClientSecretDraft(e.target.value)}
                autoComplete="off"
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterDropboxClientSecretHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterDropboxPath')}
              <input
                type="text"
                value={dropboxPathDraft}
                onChange={(e) => setDropboxPathDraft(e.target.value)}
                placeholder="/aperio"
                spellCheck={false}
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterDropboxPathHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__actions">
            <button
              type="button"
              disabled={busyDropboxOauth}
              onClick={() => void onConnectDropbox()}
            >
              {busyDropboxOauth
                ? t('dialogs.settings.sync.adapterDropboxSigningIn')
                : dropboxSignedIn
                  ? t('dialogs.settings.sync.adapterDropboxResignIn')
                  : t('dialogs.settings.sync.adapterDropboxSignIn')}
            </button>
            {dropboxSignedIn && (
              <span className="sync-panel__hint" role="status" aria-live="polite">
                {t('dialogs.settings.sync.adapterDropboxSignedIn')}
              </span>
            )}
          </div>
        </>
      )}
      {kindDraft === 'googledrive' && (
        <>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.adapterGoogledriveIntro')}
          </FocusableNote>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterGoogledriveClientId')}
              <input
                type="text"
                value={gdriveClientIdDraft}
                onChange={(e) => setGdriveClientIdDraft(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterGoogledriveClientIdHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterGoogledriveClientSecret')}
              <input
                type="password"
                value={gdriveClientSecretDraft}
                onChange={(e) => setGdriveClientSecretDraft(e.target.value)}
                autoComplete="off"
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterGoogledriveClientSecretHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterGoogledriveFolderName')}
              <input
                type="text"
                value={gdriveFolderNameDraft}
                onChange={(e) => setGdriveFolderNameDraft(e.target.value)}
                placeholder="Aperio"
                spellCheck={false}
              />
            </label>
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterGoogledriveFolderNameHint')}
            </FocusableNote>
          </div>
          <div className="sync-panel__actions">
            <button
              type="button"
              disabled={busyGdriveOauth}
              onClick={() => void onConnectGoogledrive()}
            >
              {busyGdriveOauth
                ? t('dialogs.settings.sync.adapterGoogledriveSigningIn')
                : gdriveSignedIn
                  ? t('dialogs.settings.sync.adapterGoogledriveResignIn')
                  : t('dialogs.settings.sync.adapterGoogledriveSignIn')}
            </button>
            {gdriveSignedIn && (
              <span className="sync-panel__hint" role="status" aria-live="polite">
                {t('dialogs.settings.sync.adapterGoogledriveSignedIn')}
              </span>
            )}
          </div>
        </>
      )}
      {/* Device name — registers this device when joining or initializing. */}
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
        <FocusableNote className="sync-panel__hint">
          {t('dialogs.settings.sync.deviceNameHint')}
        </FocusableNote>
      </div>

      {/* Inputs the connect preview reveals on demand. */}
      {preview?.kind === 'empty' && (
        <E2eEnableInput
          enabled={enableE2eDraft}
          onToggle={setEnableE2eDraft}
          passphrase={passphraseDraft}
          onPassphraseChange={setPassphraseDraft}
          t={t}
        />
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
          <SyncExistingInfo preview={preview} t={t} fmt={fmt} />
        </>
      )}

      <div className="sync-panel__actions">
        <button
          type="button"
          disabled={busyAdapter}
          onClick={() => void onConnect()}
        >
          {busyAdapter
            ? t('dialogs.settings.sync.adapterConnecting')
            : t('dialogs.settings.sync.adapterConfigure')}
        </button>
        {kindDraft !== 'none' && (
          <button
            type="button"
            disabled={busyAdapter || busyTest}
            onClick={() => void onTest()}
          >
            {busyTest
              ? t('dialogs.settings.sync.adapterTesting')
              : t('dialogs.settings.sync.adapterTest')}
          </button>
        )}
      </div>

      {preview?.kind === 'existing' && (
        <button
          type="button"
          className="sync-panel__overwrite form__action--danger"
          disabled={busyAdopt}
          onClick={() => void onOverwrite()}
        >
          {t('dialogs.settings.sync.previewAdoptButton')}
        </button>
      )}

      {adapterFeedback && (
        <p
          className={
            adapterFeedback.kind === 'error' ? 'form__error' : 'form__hint'
          }
          role={adapterFeedback.kind === 'error' ? 'alert' : 'status'}
        >
          {adapterFeedback.message}
        </p>
      )}

      <SyncSftpTrustDialog
        isOpen={trustPreview !== null}
        preview={trustPreview}
        onAccept={(fp) => void onTrustAccept(fp)}
        onCancel={onTrustCancel}
      />
    </>
  );
}

/** Read-only summary of what's already in an existing remote dataset. */
function SyncExistingInfo({
  preview,
  t,
  fmt,
}: {
  preview: Extract<SyncPreview, { kind: 'existing' }>;
  t: ReturnType<typeof useTranslation>['t'];
  fmt: ReturnType<typeof useDateFormat>;
}) {
  const summary =
    preview.snapshot_timestamp !== null
      ? t('dialogs.settings.sync.previewExisting', {
          time: (() => {
            try {
              return fmt.format(
                new Date(preview.snapshot_timestamp as string),
                'PPP',
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
      <FocusableNote>{summary}</FocusableNote>
      <FocusableNote>
        {t('dialogs.settings.sync.previewDevices', {
          count: preview.devices.length,
          names,
        })}
      </FocusableNote>
    </div>
  );
}

/** Optional "enable encryption" sub-form for an empty target (adopt_local). */
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
      <FocusableNote className="sync-panel__hint">
        {t('dialogs.settings.sync.e2eEnableHint')}
      </FocusableNote>
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
          <FocusableNote className="sync-panel__hint sync-panel__hint--warning">
            {t('dialogs.settings.sync.e2eIrreversibleWarning')}
          </FocusableNote>
        </div>
      )}
    </div>
  );
}

/** Passphrase prompt for joining an E2E-encrypted dataset (accept_remote). */
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
      <FocusableNote>
        {t('dialogs.settings.sync.e2eRemoteRequiresPassphrase')}
      </FocusableNote>
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
