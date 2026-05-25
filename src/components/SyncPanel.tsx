import { useCallback, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import { FocusableNote } from '../a11y/FocusableNote';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

import {
  acceptRemoteDataset,
  adoptLocalDataset,
  adoptRemoteEncryption,
  changeSyncPassphrase,
  compactNow,
  configureSyncAdapter,
  connectDropboxOauth,
  connectGoogledriveOauth,
  disableSyncEncryption,
  enableSyncEncryption,
  forgetSftpHostKey,
  getPinnedSftpHostKey,
  getSyncAdapterSummary,
  hasDropboxRefreshToken,
  hasGoogledriveRefreshToken,
  isCommandError,
  previewSftpHostKey,
  previewSyncTarget,
  setSyncInterval,
  testSyncAdapter,
  trustSftpHostKey,
  type HostKeyPreview,
  type SyncAdapterConfig,
  type SyncAdapterSummary,
  type SyncPreview,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/DialogState';
import { useSync } from '../state/useSync';
import { fetchAccountsNeedingConnect } from './SyncAccountsConnectDialog';
import { SyncProtocolSection } from './SyncProtocolSection';
import { SyncSftpTrustDialog } from './SyncSftpTrustDialog';

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
  const { openSyncConflicts, openSyncAccountsConnect } = useDialogState();
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
  const protocolHeadingId = useId();
  const passphraseHeadingId = useId();
  // §19.7 — heading id for the "enable encryption on an existing
  // (unencrypted) dataset" section. Mirrors `passphraseHeadingId`
  // but the section it labels only appears in the inverse state
  // (configured && !e2e_enabled).
  const enableE2eHeadingId = useId();

  // Adapter draft state. Seeded from current backend state on mount
  // so the inputs reflect the persisted choice.
  const [kindDraft, setKindDraft] = useState<
    'local' | 'webdav' | 'sftp' | 'ftp' | 'dropbox' | 'googledrive' | 'none'
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
  // FTPS-only fields. Mirrors the SFTP shape but flatter (no
  // SSH-key auth, no host-key TOFU). Port default flips with
  // the mode dropdown: 21 for explicit, 990 for implicit.
  const [ftpHostDraft, setFtpHostDraft] = useState('');
  const [ftpPortDraft, setFtpPortDraft] = useState('21');
  const [ftpUserDraft, setFtpUserDraft] = useState('');
  const [ftpPathDraft, setFtpPathDraft] = useState('');
  const [ftpPasswordDraft, setFtpPasswordDraft] = useState('');
  const [ftpModeDraft, setFtpModeDraft] = useState<
    'explicit' | 'implicit' | 'plain'
  >('explicit');
  // Dropbox-only fields. `clientId` / `clientSecret` are from
  // the user's own app at dropbox.com/developers/apps;
  // `client_secret` stays empty for public PKCE-only apps. The
  // refresh token doesn't surface here — it lives in the OS
  // keychain after the OAuth dance completes.
  const [dropboxClientIdDraft, setDropboxClientIdDraft] = useState('');
  const [dropboxClientSecretDraft, setDropboxClientSecretDraft] =
    useState('');
  const [dropboxPathDraft, setDropboxPathDraft] = useState('');
  const [busyDropboxOauth, setBusyDropboxOauth] = useState(false);
  /** Whether a refresh token is already in the keychain.
   *  Refreshed on mount + after the OAuth round-trip finishes
   *  so the "Sign in" button flips to "Re-sign in" once auth
   *  has happened. */
  const [dropboxSignedIn, setDropboxSignedIn] = useState(false);
  // Google Drive-only fields. Same shape as the Dropbox block,
  // but Google requires both `client_id` and `client_secret`
  // for installed apps and addresses files by ID rather than
  // path — the user picks a folder name instead of a path.
  const [gdriveClientIdDraft, setGdriveClientIdDraft] = useState('');
  const [gdriveClientSecretDraft, setGdriveClientSecretDraft] =
    useState('');
  const [gdriveFolderNameDraft, setGdriveFolderNameDraft] = useState('');
  const [busyGdriveOauth, setBusyGdriveOauth] = useState(false);
  /** Whether a Google refresh token is already in the keychain.
   *  Same role as `dropboxSignedIn` — toggles the OAuth button
   *  between "Sign in" / "Re-sign in" / "Signed in ✓". */
  const [gdriveSignedIn, setGdriveSignedIn] = useState(false);
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
  const [busyTest, setBusyTest] = useState(false);
  const [busyPreview, setBusyPreview] = useState(false);
  const [busyAccept, setBusyAccept] = useState(false);
  const [busyAdopt, setBusyAdopt] = useState(false);
  const [busyCompact, setBusyCompact] = useState(false);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  // Phase Sm-3: SFTP host-key trust dialog. `trustPreview` holds
  // the snapshot the backend returned from `previewSftpHostKey`;
  // when non-null and `status.kind !== 'unchanged'` the dialog is
  // open. `pendingSftpConfig` carries the configure payload that
  // was on the wire when we paused for the trust gesture — once
  // the user accepts, we commit the pin then resume configure
  // with this same payload.
  const [trustPreview, setTrustPreview] = useState<HostKeyPreview | null>(
    null,
  );
  const [pendingSftpConfig, setPendingSftpConfig] =
    useState<SyncAdapterConfig | null>(null);
  // The fingerprint currently pinned for the host:port the user
  // has typed into the SFTP fields. Lets the SyncPanel render a
  // "Pin vergessen" button when one exists, without probing the
  // server. Refreshed whenever host/port change or after a
  // trust/forget gesture so the UI doesn't go stale.
  const [pinnedFingerprint, setPinnedFingerprint] = useState<string | null>(
    null,
  );
  // §19.7 passphrase rotation. `oldPassphraseDraft` is what the
  // user types to authorise the change; `newPassphraseDraft`
  // becomes the new wrap key after a successful round-trip.
  // Both clear on success so a stray window-leave doesn't leave
  // them sitting in memory. `passphraseChangeError` /
  // `passphraseChangeOk` drive the visible feedback line in the
  // section.
  const [oldPassphraseDraft, setOldPassphraseDraft] = useState('');
  const [newPassphraseDraft, setNewPassphraseDraft] = useState('');
  const [busyPassphraseChange, setBusyPassphraseChange] = useState(false);
  const [passphraseChangeError, setPassphraseChangeError] = useState<
    string | null
  >(null);
  const [passphraseChangeOk, setPassphraseChangeOk] = useState(false);
  // §19.7 disable-E2E flow. Reuses the same `oldPassphraseDraft`
  // input (top of the section) — the user types their current
  // passphrase once and can either change it or turn encryption
  // off entirely. `busyDisable` gates the button + the change
  // button so the two flows can't fire concurrently.
  const [busyDisable, setBusyDisable] = useState(false);
  const [disableError, setDisableError] = useState<string | null>(null);
  // §19.7 enable-E2E flow on an already-configured but
  // unencrypted dataset. `enableNewPpDraft` is the passphrase the
  // user picks; success migrates every blob on the remote to
  // ciphertext. The button stays separate from the
  // passphrase-change section because they're mutually exclusive
  // (one renders only when e2e is OFF, the other only when it's
  // ON).
  const [enableNewPpDraft, setEnableNewPpDraft] = useState('');
  const [busyEnable, setBusyEnable] = useState(false);
  const [enableError, setEnableError] = useState<string | null>(null);
  const [enableOk, setEnableOk] = useState(false);
  // §19.7 cross-device adoption. State for the banner that
  // appears when another device flipped encryption on and our
  // next sync round failed with `last_error_code =
  // encryption_required`. The user types the dataset passphrase
  // once; backend unlocks the DEK + swaps adapters; a follow-up
  // sync_now should succeed without re-onboarding.
  const [adoptRemotePpDraft, setAdoptRemotePpDraft] = useState('');
  const [busyAdoptRemote, setBusyAdoptRemote] = useState(false);
  const [adoptRemoteError, setAdoptRemoteError] = useState<
    string | null
  >(null);
  // Compact non-secret summary of the persisted adapter config.
  // Rendered in place of the full editable form when
  // `status?.configured`, so the "you can have multiple
  // adapters" reading of the editable form goes away. `null`
  // when no adapter is configured (the form takes over).
  const [adapterSummary, setAdapterSummary] =
    useState<SyncAdapterSummary | null>(null);

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
    if (kindDraft === 'dropbox') {
      return {
        kind: 'dropbox',
        client_id: dropboxClientIdDraft.trim(),
        // Empty string for PKCE-only public apps; the backend
        // honours both shapes.
        client_secret: dropboxClientSecretDraft.trim(),
        path: dropboxPathDraft.trim(),
      };
    }
    if (kindDraft === 'googledrive') {
      return {
        kind: 'googledrive',
        client_id: gdriveClientIdDraft.trim(),
        client_secret: gdriveClientSecretDraft.trim(),
        // Empty string lets the adapter fall back to its
        // built-in "Aperio" default — matches the backend
        // `GoogleDriveAccountConfig` contract.
        folder_name: gdriveFolderNameDraft.trim(),
      };
    }
    if (kindDraft === 'ftp') {
      // Same parse-or-default port pattern as SFTP. The
      // backend's `default_ftp_port` is 21 (explicit FTPS); we
      // honour the user's input where reasonable.
      const port = Number.parseInt(ftpPortDraft, 10);
      // Implicit defaults to 990; explicit + plain share 21
      // (AUTH TLS lives on the explicit-FTP port, plain talks
      // the same port without the upgrade command).
      const fallback = ftpModeDraft === 'implicit' ? 990 : 21;
      return {
        kind: 'ftp',
        host: ftpHostDraft.trim(),
        port: Number.isFinite(port) && port > 0 ? port : fallback,
        user: ftpUserDraft.trim(),
        path: ftpPathDraft.trim(),
        mode: ftpModeDraft,
        // Empty → backend reuses keychain entry, same
        // contract as WebDAV / SFTP.
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
    if (kindDraft === 'ftp') {
      // FTP requires host + user. Path is optional (defaults
      // to the server's home directory when blank). Password
      // can be reused from the keychain on subsequent edits.
      return !ftpHostDraft.trim() || !ftpUserDraft.trim();
    }
    if (kindDraft === 'dropbox') {
      // Dropbox requires the client_id and a completed OAuth
      // sign-in. client_secret + path are optional. Without a
      // refresh token the Connect step would build an adapter
      // that can't mint access tokens, so we gate on it here.
      return !dropboxClientIdDraft.trim() || !dropboxSignedIn;
    }
    if (kindDraft === 'googledrive') {
      // Google's installed-app flow needs both id + secret AND
      // a finished OAuth dance. Folder name is optional (the
      // adapter defaults it to "Aperio").
      return (
        !gdriveClientIdDraft.trim()
        || !gdriveClientSecretDraft.trim()
        || !gdriveSignedIn
      );
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

  // Shared tail of the configure flow used by both the non-SFTP
  // branch and the post-trust-dialog SFTP resume. Pulled out so
  // both call sites stay literally identical — the trust dialog
  // doesn't bypass any of the success bookkeeping.
  const finishConfigure = useCallback(
    async (config: SyncAdapterConfig) => {
      setBusyAdapter(true);
      try {
        await configureSyncAdapter(config);
        // Clear password fields after a successful connect so they
        // don't sit in memory longer than necessary. The keychain
        // entry is the canonical store from this point on.
        if (config.kind === 'webdav') setPasswordDraft('');
        if (config.kind === 'sftp') {
          setSftpPasswordDraft('');
          setSftpKeyPassphraseDraft('');
        }
        // Refresh the persisted-adapter summary so the
        // "Verbunden mit X" card lands immediately, without
        // waiting for the next sync-status event.
        getSyncAdapterSummary()
          .then(setAdapterSummary)
          .catch((err) => {
            // eslint-disable-next-line no-console
            console.warn('get_sync_adapter_summary failed', err);
          });
        // Auto-fire preview so the user lands directly on the
        // Adopt/Accept + E2E-checkbox step instead of having to
        // click "Datensatz prüfen" as a separate gesture (which
        // hid the E2E option entirely if they didn't realise
        // the second click was needed).
        setBusyPreview(true);
        setPreviewError(null);
        try {
          const result = await previewSyncTarget(config);
          setPreview(result);
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn('preview after configure failed', err);
          setPreviewError(messageForError(err));
          setPreview(null);
        } finally {
          setBusyPreview(false);
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
    },
    [announce, messageForError, t],
  );

  const onConfigure = useCallback(async () => {
    if (configMissingRequired) {
      announce(t('dialogs.settings.sync.adapterNeedPath'), 'assertive');
      return;
    }
    const config = buildConfig();
    // SFTP: probe the host key BEFORE committing the configure.
    // The user gets a deliberate trust gesture on first use or
    // on key rotation (§19.5). `unchanged` skips the gesture —
    // we already trust this server.
    if (config.kind === 'sftp') {
      setBusyAdapter(true);
      let previewResult: HostKeyPreview;
      try {
        previewResult = await previewSftpHostKey(config.host, config.port);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('preview_sftp_host_key failed', err);
        announce(
          `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
          'assertive',
        );
        setBusyAdapter(false);
        return;
      }
      if (previewResult.status.kind === 'unchanged') {
        // Pin matches the stored one — skip the dialog. `setBusyAdapter`
        // is reset inside `finishConfigure`.
        setBusyAdapter(false);
        await finishConfigure(config);
        return;
      }
      // Hand off to the dialog. Park the bespoke config so the
      // accept handler can call configure with exactly the same
      // payload (incl. password / key bits) the user submitted.
      setBusyAdapter(false);
      setPendingSftpConfig(config);
      setTrustPreview(previewResult);
      return;
    }
    // Non-SFTP backends configure directly.
    await finishConfigure(config);
  }, [
    announce,
    buildConfig,
    configMissingRequired,
    finishConfigure,
    messageForError,
    t,
  ]);

  // Trust dialog: user accepted the fingerprint. Pin it via the
  // backend then resume the parked configure call.
  const onTrustAccept = useCallback(
    async (fingerprint: string) => {
      const config = pendingSftpConfig;
      const preview = trustPreview;
      // Close the dialog before the configure round so the
      // backdrop / inert doesn't hover over the spinner.
      setTrustPreview(null);
      setPendingSftpConfig(null);
      if (!config || !preview) return;
      try {
        await trustSftpHostKey(preview.host_port, fingerprint);
        // Reflect the new pin in the UI immediately so the
        // "Vergessen" button shows up without waiting for the
        // host/port effect to re-fire.
        setPinnedFingerprint(fingerprint);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('trust_sftp_host_key failed', err);
        announce(
          `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
          'assertive',
        );
        return;
      }
      await finishConfigure(config);
    },
    [
      announce,
      finishConfigure,
      messageForError,
      pendingSftpConfig,
      t,
      trustPreview,
    ],
  );

  const onTrustCancel = useCallback(() => {
    setTrustPreview(null);
    setPendingSftpConfig(null);
  }, []);

  // "Verbindung testen" — build the adapter, run `test_connection`,
  // throw the handle away. Never persists, never mutates the
  // active orchestrator. Errors are announced verbatim via the
  // standard error path; success gets a brief "OK" announcement.
  const onTest = useCallback(async () => {
    if (configMissingRequired) {
      announce(t('dialogs.settings.sync.adapterNeedPath'), 'assertive');
      return;
    }
    setBusyTest(true);
    try {
      await testSyncAdapter(buildConfig());
      announce(t('dialogs.settings.sync.adapterTestOk'));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('test_sync_adapter failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyTest(false);
    }
  }, [
    announce,
    buildConfig,
    configMissingRequired,
    messageForError,
    t,
  ]);

  const onDisconnect = useCallback(async () => {
    setBusyAdapter(true);
    try {
      await configureSyncAdapter({ kind: 'none' });
      setPreview(null);
      setAdapterSummary(null);
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
      // §19.11 step 8: the snapshot we just applied may have
      // brought in external account rows whose secrets are not
      // on this device yet. Open the reconnect wizard so the
      // user can re-attach credentials in one go. Skip silently
      // when there's nothing to do (Local-only datasets, or the
      // user already has every secret in their keychain).
      const needConnect = await fetchAccountsNeedingConnect();
      if (needConnect && needConnect.length > 0) {
        openSyncAccountsConnect(needConnect);
      }
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
    openSyncAccountsConnect,
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

  // Native directory picker for the local-FS adapter path.
  // Same plugin / failure-mode contract as `onBrowseKey` below —
  // the text input keeps working if the dialog plugin is
  // unavailable.
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

  // Native file picker for the SSH key path. Uses
  // `tauri-plugin-dialog` so the user gets the platform-native
  // picker idiom; falls back silently on the (unlikely) error so
  // the bare text input remains usable.
  const onBrowseKey = useCallback(async () => {
    try {
      const selected = await openFileDialog({
        multiple: false,
        directory: false,
        title: t('dialogs.settings.sync.adapterSftpKeyPathDialogTitle'),
        // No platform-specific extension filters — SSH keys
        // commonly carry no extension (`id_ed25519`) or `.pem`
        // depending on origin. A filter would hide the file the
        // user is looking for as often as it would help.
      });
      // `open` returns `string | string[] | null`. With
      // `multiple: false` we only ever see a single path or null.
      if (typeof selected === 'string' && selected.length > 0) {
        setSftpKeyPathDraft(selected);
      }
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('SSH key picker failed', err);
    }
  }, [t]);

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

  // §19.7 — drive the passphrase change. Inline validation
  // first (both fields filled, new differs from old), then call
  // the backend. Empties the inputs on success so they don't
  // sit in memory; surfaces auth failures from the underlying
  // wrap unwrap as the "wrong current passphrase" message.
  const onChangePassphrase = useCallback(async () => {
    setPassphraseChangeOk(false);
    setPassphraseChangeError(null);
    const oldPp = oldPassphraseDraft.trim();
    const newPp = newPassphraseDraft.trim();
    if (!oldPp || !newPp) {
      setPassphraseChangeError(
        t('dialogs.settings.sync.passphraseChangeErrorEmpty'),
      );
      return;
    }
    if (oldPp === newPp) {
      setPassphraseChangeError(
        t('dialogs.settings.sync.passphraseChangeErrorSame'),
      );
      return;
    }
    setBusyPassphraseChange(true);
    try {
      await changeSyncPassphrase(oldPp, newPp);
      setPassphraseChangeOk(true);
      setOldPassphraseDraft('');
      setNewPassphraseDraft('');
      announce(t('dialogs.settings.sync.passphraseChangeOk'));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('change_sync_passphrase failed', err);
      // Map `auth` to a user-friendly "wrong current passphrase"
      // message; everything else goes through the standard
      // messageForError translation. The auth case is by far the
      // most likely user-visible failure.
      if (isCommandError(err) && err.code === 'auth') {
        setPassphraseChangeError(
          t('dialogs.settings.sync.passphraseChangeErrorAuth'),
        );
      } else {
        setPassphraseChangeError(messageForError(err));
      }
    } finally {
      setBusyPassphraseChange(false);
    }
  }, [
    announce,
    messageForError,
    newPassphraseDraft,
    oldPassphraseDraft,
    t,
  ]);

  // §19.7 — turn off E2E on the dataset. Gated by the same
  // "current passphrase" input as the change flow, plus a
  // window.confirm whose message names the cluster-wide
  // consequence (other devices need to re-onboard). Success
  // clears the inputs + announces the outcome; failure surfaces
  // inline.
  const onDisableEncryption = useCallback(async () => {
    setDisableError(null);
    const oldPp = oldPassphraseDraft.trim();
    if (!oldPp) {
      setDisableError(
        t('dialogs.settings.sync.disableE2eErrorNeedsPassphrase'),
      );
      return;
    }
    const confirmed = window.confirm(
      t('dialogs.settings.sync.disableE2eConfirm'),
    );
    if (!confirmed) return;
    setBusyDisable(true);
    try {
      const report = await disableSyncEncryption(oldPp);
      setOldPassphraseDraft('');
      setNewPassphraseDraft('');
      announce(
        t('dialogs.settings.sync.disableE2eOkAnnouncement', {
          logs: report.logs_rewritten,
        }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('disable_sync_encryption failed', err);
      if (isCommandError(err) && err.code === 'auth') {
        setDisableError(
          t('dialogs.settings.sync.passphraseChangeErrorAuth'),
        );
      } else {
        setDisableError(messageForError(err));
      }
    } finally {
      setBusyDisable(false);
    }
  }, [announce, messageForError, oldPassphraseDraft, t]);

  // §19.7 — turn ON encryption for a dataset that was originally
  // adopted without it. Mirrors `onDisableEncryption` in shape:
  // validate the new passphrase, gate behind a window.confirm
  // (the bulk re-encryption is destructive of the remote's
  // plaintext copies + other devices need the new passphrase to
  // keep syncing), then call into the backend. Success clears
  // the input + flips the inline ok message; failure surfaces
  // the message.
  const onEnableEncryption = useCallback(async () => {
    setEnableError(null);
    setEnableOk(false);
    const newPp = enableNewPpDraft.trim();
    if (!newPp) {
      setEnableError(
        t('dialogs.settings.sync.enableE2eErrorNeedsPassphrase'),
      );
      return;
    }
    const confirmed = window.confirm(
      t('dialogs.settings.sync.enableE2eConfirm'),
    );
    if (!confirmed) return;
    setBusyEnable(true);
    try {
      const report = await enableSyncEncryption(newPp);
      setEnableNewPpDraft('');
      setEnableOk(true);
      announce(
        t('dialogs.settings.sync.enableE2eOkAnnouncement', {
          logs: report.logs_rewritten,
        }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('enable_sync_encryption failed', err);
      // `conflict` is the "another device flipped it first" race;
      // surface a dedicated hint so the user knows to re-onboard
      // via the standard passphrase prompt instead of retrying
      // this command.
      if (isCommandError(err) && err.code === 'conflict') {
        setEnableError(
          t('dialogs.settings.sync.enableE2eErrorConflict'),
        );
      } else {
        setEnableError(messageForError(err));
      }
    } finally {
      setBusyEnable(false);
    }
  }, [announce, enableNewPpDraft, messageForError, t]);

  // §19.7 — adopt encryption that was activated on another
  // device. Triggered from the cross-device banner that mounts
  // when local thinks e2e is off but the last sync round failed
  // with `encryption_required` (= remote meta says it's on).
  // Pure unlock; a `kick()`-style refresh after success would be
  // nice but a manual "Sync now" by the user is also fine since
  // the orchestrator is already swapped over.
  const onAdoptRemoteEncryption = useCallback(async () => {
    setAdoptRemoteError(null);
    const pp = adoptRemotePpDraft.trim();
    if (!pp) {
      setAdoptRemoteError(
        t('dialogs.settings.sync.enableE2eErrorNeedsPassphrase'),
      );
      return;
    }
    setBusyAdoptRemote(true);
    try {
      await adoptRemoteEncryption(pp);
      setAdoptRemotePpDraft('');
      announce(t('dialogs.settings.sync.adoptRemoteE2eOk'));
      // Kick a fresh sync round so the indicator + lastError
      // clear without the user having to click Sync now.
      void triggerSync();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('adopt_remote_encryption failed', err);
      if (isCommandError(err) && err.code === 'auth') {
        setAdoptRemoteError(
          t('dialogs.settings.sync.passphraseChangeErrorAuth'),
        );
      } else {
        setAdoptRemoteError(messageForError(err));
      }
    } finally {
      setBusyAdoptRemote(false);
    }
  }, [adoptRemotePpDraft, announce, messageForError, t, triggerSync]);

  // §19.6 — Dropbox OAuth handlers.
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
    const clientId = dropboxClientIdDraft.trim();
    if (!clientId) {
      announce(
        t('dialogs.settings.sync.adapterDropboxNeedsClientId'),
        'assertive',
      );
      return;
    }
    setBusyDropboxOauth(true);
    try {
      await connectDropboxOauth(
        clientId,
        dropboxClientSecretDraft.trim(),
      );
      announce(t('dialogs.settings.sync.adapterDropboxSignedInAnnouncement'));
      refreshDropboxSignedIn();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('connect_dropbox_oauth failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
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

  // §19.6 — Google Drive OAuth handlers. Same flow as Dropbox
  // (probe on mount, run the dance, refresh the flag) but with
  // a different missing-input announcement: Google needs both
  // id AND secret before the dance can even start.
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
    const clientId = gdriveClientIdDraft.trim();
    const clientSecret = gdriveClientSecretDraft.trim();
    if (!clientId || !clientSecret) {
      announce(
        t('dialogs.settings.sync.adapterGoogledriveNeedsClientId'),
        'assertive',
      );
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
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
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

  // Look up the currently-pinned fingerprint whenever the SFTP
  // host/port pair changes — so the "Aktueller Pin" line + the
  // "Vergessen" button update without a manual refresh. Debounced
  // via the natural typing cadence; an extra short delay isn't
  // worth the complexity for a fingerprint readout.
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

  // Reload the persisted-adapter summary. Called on mount, after
  // configure succeeds (form → summary), and after disconnect
  // (summary → form). Cheap: no IO, just a few pref reads.
  const refreshAdapterSummary = useCallback(() => {
    getSyncAdapterSummary()
      .then(setAdapterSummary)
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('get_sync_adapter_summary failed', err);
        setAdapterSummary(null);
      });
  }, []);

  useEffect(() => {
    refreshAdapterSummary();
  }, [refreshAdapterSummary, status?.configured]);

  // "Pin vergessen" — drop the stored fingerprint so the next
  // connect goes through the first-use trust dialog again. Used
  // when the user knows their server key was rotated and wants
  // to avoid the mismatch warning on the next round.
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

  return (
    <div className="sync-panel">
      <section aria-labelledby={stateHeadingId}>
        <h3 id={stateHeadingId}>{t('dialogs.settings.sync.stateTitle')}</h3>
        <FocusableNote className="sync-panel__hint">
          {status?.configured
            ? t('dialogs.settings.sync.stateConfiguredSimple')
            : t('dialogs.settings.sync.stateUnconfigured')}
        </FocusableNote>
        {status?.configured && (
          <FocusableNote className="sync-panel__hint">
            {status.e2e_enabled
              ? t('dialogs.settings.sync.e2eActive')
              : t('dialogs.settings.sync.e2eInactive')}
          </FocusableNote>
        )}
        <FocusableNote className="sync-panel__hint">{lastSyncedLabel}</FocusableNote>
        {lastReport && (
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.lastReport', {
              pushed: lastReport.pushed_logs,
              fetched: lastReport.fetched_logs,
              applied: lastReport.applied,
            })}
          </FocusableNote>
        )}
        {lastError && (
          <p className="sync-panel__error" role="alert">
            {t('dialogs.settings.sync.errorPrefix')}: {lastError}
          </p>
        )}
        {status?.last_error_code === 'auth' && (
          <p className="sync-panel__warning" role="status">
            {t('dialogs.settings.sync.authFailureBanner')}
          </p>
        )}
        {status?.sustained_failure && (
          <p className="sync-panel__warning" role="status">
            {t('dialogs.settings.sync.sustainedFailureBanner')}
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
              {/* `count` lets i18next pick `_one` / `_other`
                  automatically; `minutes` is the actual
                  interpolation. Reads identically to the manual
                  ternary this replaced. */}
              {t('dialogs.settings.sync.intervalOption', {
                count: min,
                minutes: min,
              })}
            </option>
          ))}
        </select>
      </section>

      <section aria-labelledby={adapterHeadingId}>
        <h3 id={adapterHeadingId}>{t('dialogs.settings.sync.adapterTitle')}</h3>
        {/* When configured: show a non-editable summary card plus
            a single Disconnect button. The full form below is
            hidden so the UI no longer reads as "you can have
            multiple adapters" / "type into these fields to
            switch". To swap adapters, the user has to disconnect
            first. */}
        {status?.configured && adapterSummary ? (
          <div className="sync-panel__connected-summary">
            <FocusableNote className="sync-panel__hint">
              {t('dialogs.settings.sync.connectedSummary', {
                kind: t(`dialogs.settings.sync.adapterKind${
                  adapterSummary.kind.charAt(0).toUpperCase() +
                  adapterSummary.kind.slice(1)
                }`),
                detail: adapterSummary.detail || '–',
              })}
            </FocusableNote>
            <div className="sync-panel__actions">
              <button
                type="button"
                disabled={busyAdapter}
                onClick={() => void onDisconnect()}
              >
                {t('dialogs.settings.sync.adapterDisconnect')}
              </button>
            </div>
          </div>
        ) : (
          <>
        <FocusableNote className="sync-panel__hint">
          {t('dialogs.settings.sync.adapterBody')}
        </FocusableNote>
        <div className="sync-panel__field">
          <label>
            {t('dialogs.settings.sync.adapterKind')}
            <select
              value={kindDraft}
              onChange={(e) =>
                setKindDraft(
                  e.target.value as
                    | 'local'
                    | 'webdav'
                    | 'sftp'
                    | 'ftp'
                    | 'dropbox'
                    | 'googledrive'
                    | 'none',
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
                  aria-label={t(
                    'dialogs.settings.sync.adapterPathBrowseAria',
                  )}
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
                        onChange={(e) =>
                          setSftpKeyPathDraft(e.target.value)
                        }
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
                  <FocusableNote className="sync-panel__hint">
                    {t(
                      'dialogs.settings.sync.adapterSftpKeyPassphraseHint',
                    )}
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
              <legend>
                {t('dialogs.settings.sync.adapterFtpMode')}
              </legend>
              <label>
                <input
                  type="radio"
                  name="ftp-mode"
                  value="explicit"
                  checked={ftpModeDraft === 'explicit'}
                  onChange={() => {
                    setFtpModeDraft('explicit');
                    // Swap the port default if the user
                    // hasn't customised it away from the
                    // implicit value yet.
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
                    // Plain shares the explicit FTPS port
                    // (server-side they're the same listener;
                    // plain just skips AUTH TLS).
                    if (ftpPortDraft === '990') setFtpPortDraft('21');
                  }}
                />{' '}
                {t('dialogs.settings.sync.adapterFtpModePlain')}
              </label>
              <FocusableNote className="sync-panel__hint">
                {t('dialogs.settings.sync.adapterFtpModeHint')}
              </FocusableNote>
              {/* Plain mode gets an additional, stronger
                  warning rendered as role="alert" so the
                  user understands the privacy trade-off
                  before they click Connect. */}
              {ftpModeDraft === 'plain' && (
                <p
                  className="sync-panel__warning"
                  role="alert"
                >
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
                      ? t(
                          'dialogs.settings.sync.adapterWebdavPasswordKept',
                        )
                      : undefined
                  }
                />
              </label>
              {/* The TLS-recommended hint is purely
                  informational; with the Plain radio
                  available the user might genuinely want to
                  pick it for a LAN setup, but the surrounding
                  warning above makes the trade-off
                  explicit. */}
              <FocusableNote className="sync-panel__hint">
                {ftpModeDraft === 'plain'
                  ? t(
                      'dialogs.settings.sync.adapterFtpPlainPasswordHint',
                    )
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
                  onChange={(e) =>
                    setDropboxClientIdDraft(e.target.value)
                  }
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
                  onChange={(e) =>
                    setDropboxClientSecretDraft(e.target.value)
                  }
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
                <span
                  className="sync-panel__hint"
                  role="status"
                  aria-live="polite"
                >
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
                  onChange={(e) =>
                    setGdriveClientIdDraft(e.target.value)
                  }
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
              <FocusableNote className="sync-panel__hint">
                {t(
                  'dialogs.settings.sync.adapterGoogledriveClientIdHint',
                )}
              </FocusableNote>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterGoogledriveClientSecret')}
                <input
                  type="password"
                  value={gdriveClientSecretDraft}
                  onChange={(e) =>
                    setGdriveClientSecretDraft(e.target.value)
                  }
                  autoComplete="off"
                />
              </label>
              <FocusableNote className="sync-panel__hint">
                {t(
                  'dialogs.settings.sync.adapterGoogledriveClientSecretHint',
                )}
              </FocusableNote>
            </div>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adapterGoogledriveFolderName')}
                <input
                  type="text"
                  value={gdriveFolderNameDraft}
                  onChange={(e) =>
                    setGdriveFolderNameDraft(e.target.value)
                  }
                  placeholder="Aperio"
                  spellCheck={false}
                />
              </label>
              <FocusableNote className="sync-panel__hint">
                {t(
                  'dialogs.settings.sync.adapterGoogledriveFolderNameHint',
                )}
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
                <span
                  className="sync-panel__hint"
                  role="status"
                  aria-live="polite"
                >
                  {t('dialogs.settings.sync.adapterGoogledriveSignedIn')}
                </span>
              )}
            </div>
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
          {/* "Verbindung testen" lets the user verify host / URL /
              credentials without persisting anything. Disabled for
              the `none` kind (nothing to test) and while a real
              configure is in flight. */}
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
          </>
        )}
      </section>

      {/* Preview/onboarding stays visible while a preview result
          is in hand, even after `status.configured` flips to true
          (the auto-preview that fires after configure relies on
          this — the user lands directly on the Adopt/Accept +
          E2E checkbox without losing the section). */}
      {(!status?.configured || preview !== null) && (
        <section aria-labelledby={previewHeadingId}>
          <h3 id={previewHeadingId}>
            {t('dialogs.settings.sync.previewTitle')}
          </h3>
          <FocusableNote>{t('dialogs.settings.sync.previewBody')}</FocusableNote>
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
          {/* Trust dialog is mounted as part of the panel so it can
              reach into the `pendingSftpConfig` + `trustPreview`
              state without going through the global DialogState
              stack. See `SyncSftpTrustDialog` for the rationale. */}
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
      {/* §19.7 — cross-device adoption banner. Appears when
          local thinks E2E is off but the last sync round failed
          with `encryption_required` (= another device just
          flipped meta.json to `e2e_enabled = true`). The user
          enters the dataset passphrase; the backend unlocks the
          DEK + swaps adapters; the follow-up sync_now resumes
          syncing transparently. */}
      {status?.configured &&
        !status?.e2e_enabled &&
        status?.last_error_code === 'encryption_required' && (
          <section
            className="sync-panel__remote-e2e-banner"
            role="alert"
          >
            <h3>
              {t('dialogs.settings.sync.adoptRemoteE2eTitle')}
            </h3>
            <FocusableNote>{t('dialogs.settings.sync.adoptRemoteE2eHint')}</FocusableNote>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
                <input
                  type="password"
                  value={adoptRemotePpDraft}
                  onChange={(e) => setAdoptRemotePpDraft(e.target.value)}
                  autoComplete="current-password"
                />
              </label>
            </div>
            {adoptRemoteError && (
              <p className="sync-panel__error" role="alert">
                {t('dialogs.settings.sync.errorPrefix')}:{' '}
                {adoptRemoteError}
              </p>
            )}
            <div className="sync-panel__actions">
              <button
                type="button"
                disabled={busyAdoptRemote}
                onClick={() => void onAdoptRemoteEncryption()}
              >
                {busyAdoptRemote
                  ? t('dialogs.settings.sync.adoptRemoteE2eRunning')
                  : t('dialogs.settings.sync.adoptRemoteE2eAction')}
              </button>
            </div>
          </section>
        )}
      {/* §19.7 — turn on encryption for an existing,
          unencrypted dataset. Only visible when the dataset is
          configured but `e2e_enabled` is false. Mirror image of
          the passphrase-change section below: never both at
          once. */}
      {status?.configured && !status?.e2e_enabled && (
        <section aria-labelledby={enableE2eHeadingId}>
          <h3 id={enableE2eHeadingId}>
            {t('dialogs.settings.sync.enableE2eTitle')}
          </h3>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.enableE2eHint')}
          </FocusableNote>
          <FocusableNote className="sync-panel__hint sync-panel__hint--warning">
            {t('dialogs.settings.sync.enableE2eMultiDeviceWarning')}
          </FocusableNote>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.enableE2ePassphraseLabel')}
              <input
                type="password"
                value={enableNewPpDraft}
                onChange={(e) => setEnableNewPpDraft(e.target.value)}
                autoComplete="new-password"
              />
            </label>
          </div>
          {enableError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {enableError}
            </p>
          )}
          {enableOk && (
            <p className="sync-panel__hint" role="status">
              {t('dialogs.settings.sync.enableE2eOk')}
            </p>
          )}
          <div className="sync-panel__actions">
            <button
              type="button"
              disabled={busyEnable}
              onClick={() => void onEnableEncryption()}
            >
              {busyEnable
                ? t('dialogs.settings.sync.enableE2eRunning')
                : t('dialogs.settings.sync.enableE2eAction')}
            </button>
          </div>
        </section>
      )}
      {/* §19.7 — change passphrase. Only visible when the
          dataset is actually encrypted and configured; a
          non-E2E or unconfigured sync setup has nothing to
          rotate. The DEK doesn't change so other devices
          keep syncing without interruption — the new
          passphrase is only needed to onboard a fresh
          device after this point. */}
      {status?.configured && status?.e2e_enabled && (
        <section aria-labelledby={passphraseHeadingId}>
          <h3 id={passphraseHeadingId}>
            {t('dialogs.settings.sync.passphraseChangeTitle')}
          </h3>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.passphraseChangeHint')}
          </FocusableNote>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.passphraseChangeOld')}
              <input
                type="password"
                value={oldPassphraseDraft}
                onChange={(e) => setOldPassphraseDraft(e.target.value)}
                autoComplete="current-password"
              />
            </label>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.passphraseChangeNew')}
              <input
                type="password"
                value={newPassphraseDraft}
                onChange={(e) => setNewPassphraseDraft(e.target.value)}
                autoComplete="new-password"
              />
            </label>
          </div>
          {passphraseChangeError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}:{' '}
              {passphraseChangeError}
            </p>
          )}
          {passphraseChangeOk && (
            <p className="sync-panel__hint" role="status">
              {t('dialogs.settings.sync.passphraseChangeOk')}
            </p>
          )}
          {disableError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {disableError}
            </p>
          )}
          <div className="sync-panel__actions">
            <button
              type="button"
              disabled={busyPassphraseChange || busyDisable}
              onClick={() => void onChangePassphrase()}
            >
              {busyPassphraseChange
                ? t('dialogs.settings.sync.passphraseChangeRunning')
                : t('dialogs.settings.sync.passphraseChangeAction')}
            </button>
            {/* "Disable encryption" — destructive, gated by the
                same "current passphrase" input + a window.confirm.
                Lives in the same section because both flows need
                the user to type their current passphrase
                first. */}
            <button
              type="button"
              disabled={busyPassphraseChange || busyDisable}
              onClick={() => void onDisableEncryption()}
            >
              {busyDisable
                ? t('dialogs.settings.sync.disableE2eRunning')
                : t('dialogs.settings.sync.disableE2eAction')}
            </button>
          </div>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.disableE2eHint')}
          </FocusableNote>
        </section>
      )}
      {/* §19.9 detailed Sync-Protokoll. Always rendered (no
          gating on `configured`) so users can still see the
          history of past attempts after a disconnect. */}
      <SyncProtocolSection headingId={protocolHeadingId} />
      <SyncSftpTrustDialog
        isOpen={trustPreview !== null}
        preview={trustPreview}
        onAccept={(fp) => void onTrustAccept(fp)}
        onCancel={onTrustCancel}
      />
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
      <FocusableNote>{t('dialogs.settings.sync.previewEmpty')}</FocusableNote>
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
      <FocusableNote>{summary}</FocusableNote>
      <FocusableNote>
        {t('dialogs.settings.sync.previewDevices', {
          count: preview.devices.length,
          names,
        })}
      </FocusableNote>
      <div className="sync-panel__preview-actions">
        <div>
          <h4>{t('dialogs.settings.sync.previewAcceptTitle')}</h4>
          <FocusableNote>{t('dialogs.settings.sync.previewAcceptBody')}</FocusableNote>
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
          <FocusableNote>{t('dialogs.settings.sync.previewAdoptBody')}</FocusableNote>
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
      <FocusableNote>{t('dialogs.settings.sync.e2eRemoteRequiresPassphrase')}</FocusableNote>
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
