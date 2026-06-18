import { useFocusEffect, useNavigation } from '@react-navigation/native';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  acceptRemoteDataset,
  adoptRemoteEncryption,
  changeSyncPassphrase,
  configureSyncAdapter,
  disableSyncEncryption,
  enableSyncEncryption,
  forgetSftpHostKey,
  previewSftpHostKey,
  previewSyncTarget,
  trustSftpHostKey,
  syncConflictCount,
  syncNow,
  syncStatus,
  HostKeyPreview,
  SyncAdapterConfig,
  SyncDeviceSummary,
  SyncPreview,
  SyncStatus,
} from '../api/sync';
import { connectSyncOAuth } from '../api/oauth';
import { RadioGroup } from '../components/RadioGroup';

// Cross-device sync — a full desktop peer (same engine, statically-embedded
// adapters). This screen exposes the password targets (a local shared folder,
// WebDAV, FTPS), the OAuth targets (Dropbox, Google Drive — BYO client-id,
// browser sign-in), and SFTP (SSH server, with the §19.5 host-key TOFU trust
// flow). Screen-reader-first: the kind is a radio group, every field is its own
// labelled stop, the trust panel is an assertive live region, results announced.

type SyncKind = 'local' | 'webdav' | 'ftp' | 'dropbox' | 'googledrive' | 'sftp';
type FtpMode = 'explicit' | 'implicit' | 'plain';
type SftpAuth = 'password' | 'key';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SyncScreen() {
  const { t } = useTranslation();
  const navigation = useNavigation();

  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [conflictCount, setConflictCount] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Which target to configure, plus the per-kind fields. `user`/`password` are
  // shared by WebDAV + FTP (only the active kind's fields are shown).
  const [kind, setKind] = useState<SyncKind>('local');
  const [path, setPath] = useState(''); // local
  const [url, setUrl] = useState(''); // webdav
  const [host, setHost] = useState(''); // ftp
  const [port, setPort] = useState(''); // ftp (blank → mode default)
  const [ftpPath, setFtpPath] = useState(''); // ftp
  const [mode, setMode] = useState<FtpMode>('explicit'); // ftp
  const [user, setUser] = useState(''); // webdav + ftp
  const [password, setPassword] = useState(''); // webdav + ftp
  const [oauthClientId, setOauthClientId] = useState(''); // dropbox + googledrive
  const [oauthClientSecret, setOauthClientSecret] = useState(''); // dropbox(opt)+gdrive
  const [dropboxPath, setDropboxPath] = useState(''); // dropbox
  const [folderName, setFolderName] = useState(''); // googledrive
  const [sftpHost, setSftpHost] = useState(''); // sftp
  const [sftpPort, setSftpPort] = useState(''); // sftp (blank → 22)
  const [sftpUser, setSftpUser] = useState(''); // sftp
  const [sftpPath, setSftpPath] = useState(''); // sftp
  const [sftpAuth, setSftpAuth] = useState<SftpAuth>('password'); // sftp
  const [sftpPassword, setSftpPassword] = useState(''); // sftp password-auth
  const [sftpKeyPath, setSftpKeyPath] = useState(''); // sftp key-auth
  const [sftpKeyPassphrase, setSftpKeyPassphrase] = useState(''); // sftp key-auth
  const [e2ePassphrase, setE2ePassphrase] = useState(''); // enable-E2E passphrase
  const [changeOldPp, setChangeOldPp] = useState(''); // rotate: current passphrase
  const [changeNewPp, setChangeNewPp] = useState(''); // rotate: new passphrase
  const [disablePp, setDisablePp] = useState(''); // disable: current passphrase (own field)
  const [adoptPp, setAdoptPp] = useState(''); // adopt peer-enabled E2E passphrase
  // The adopt banner's title — SR focus lands here when it appears so the blind
  // user reaches the passphrase prompt after the round failed.
  const adoptBannerRef = useRef<Text>(null);
  // §19.11 onboarding: the result of probing the entered target (null until the
  // user taps "Check existing dataset"). When `existing`, the join panel shows.
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [joinPassphrase, setJoinPassphrase] = useState(''); // join-existing E2E passphrase
  const [joinDeviceName, setJoinDeviceName] = useState(''); // optional name for meta.json
  // Latest preview, readable inside the invalidation effect WITHOUT making
  // `preview` a dep (which would re-run + clear it in a loop). Lets the effect
  // announce "the check is now stale" only when one was actually showing.
  const previewRef = useRef<SyncPreview | null>(null);
  previewRef.current = preview;
  // The join-panel title — SR focus lands here when the panel appears (mirrors
  // the SFTP trust panel) so the blind user reaches the passphrase/Adopt controls
  // instead of being stranded on the "Check existing dataset" button.
  const joinPanelRef = useRef<Text>(null);
  // When set, the §19.5 trust panel is showing the probed fingerprint awaiting
  // the user's explicit accept (first-use or key-change) before connect.
  const [pendingTrust, setPendingTrust] = useState<HostKeyPreview | null>(null);
  // The trust panel's title — SR focus lands here when the panel appears (each
  // child, incl. the Accept/Cancel buttons, stays its own a11y node). The connect
  // button is where focus returns after the panel closes (accept/cancel).
  const trustRef = useRef<Text>(null);
  const sftpConnectRef = useRef<View>(null);

  const focusSftpConnect = useCallback(() => {
    const tag = sftpConnectRef.current
      ? findNodeHandle(sftpConnectRef.current)
      : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  const isOAuthKind = kind === 'dropbox' || kind === 'googledrive';
  const isSftp = kind === 'sftp';
  // Sanitize the port (matching the FTP field + the desktop guard): a
  // non-numeric / out-of-range entry falls back to 22 so it never reaches the
  // bridge as NaN (which would break the host:port pin key + give an opaque
  // error). Used identically for preview, configure, and forget.
  const sftpPortParsed = Number(sftpPort.trim());
  const sftpPortNum =
    sftpPort.trim().length > 0 &&
    Number.isInteger(sftpPortParsed) &&
    sftpPortParsed > 0 &&
    sftpPortParsed <= 65535
      ? sftpPortParsed
      : 22;

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const refresh = useCallback(async () => {
    try {
      setStatus(await syncStatus());
      setConflictCount(await syncConflictCount().catch(() => 0));
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  // Refresh on every focus (not just mount) so a background-triggered round's
  // result — including the sustained-failure latch — shows when the user opens
  // this screen.
  useFocusEffect(
    useCallback(() => {
      void refresh();
    }, [refresh]),
  );

  // Move screen-reader focus onto the trust panel when it appears so the blind
  // user lands on the fingerprint they must verify (not stranded after the
  // probe). The panel is also an assertive live region.
  useEffect(() => {
    if (pendingTrust == null) return;
    const tag = trustRef.current ? findNodeHandle(trustRef.current) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [pendingTrust]);

  const kindOptions = useMemo(
    () => [
      { value: 'local' as const, label: t('dialogs.settings.sync.adapterKindLocal') },
      { value: 'webdav' as const, label: t('dialogs.settings.sync.adapterKindWebdav') },
      { value: 'ftp' as const, label: t('dialogs.settings.sync.adapterKindFtp') },
      {
        value: 'dropbox' as const,
        label: t('dialogs.settings.sync.adapterKindDropbox'),
      },
      {
        value: 'googledrive' as const,
        label: t('dialogs.settings.sync.adapterKindGoogledrive'),
      },
      { value: 'sftp' as const, label: t('dialogs.settings.sync.adapterKindSftp') },
    ],
    [t],
  );

  const sftpAuthOptions = useMemo(
    () => [
      {
        value: 'password' as const,
        label: t('dialogs.settings.sync.adapterSftpAuthPassword'),
      },
      { value: 'key' as const, label: t('dialogs.settings.sync.adapterSftpAuthKey') },
    ],
    [t],
  );

  const ftpModeOptions = useMemo(
    () => [
      { value: 'explicit' as const, label: t('dialogs.settings.sync.adapterFtpModeExplicit') },
      { value: 'implicit' as const, label: t('dialogs.settings.sync.adapterFtpModeImplicit') },
      { value: 'plain' as const, label: t('dialogs.settings.sync.adapterFtpModePlain') },
    ],
    [t],
  );

  // Build the wire config for the active kind, or null when a required field is
  // blank (the configure button stays a no-op rather than erroring). Optional
  // password is omitted when blank so the Rust side reuses the keychain secret.
  const buildConfig = useCallback((): SyncAdapterConfig | null => {
    if (kind === 'local') {
      const p = path.trim();
      return p.length > 0 ? { kind: 'local', path: p } : null;
    }
    if (kind === 'webdav') {
      const u = url.trim();
      if (u.length === 0) return null;
      return password.length > 0
        ? { kind: 'webdav', url: u, user: user.trim(), password }
        : { kind: 'webdav', url: u, user: user.trim() };
    }
    // The OAuth kinds (dropbox/googledrive) build their config inside the sign-in
    // flow, not here — buildConfig serves only the generic "Use this target".
    if (kind !== 'ftp') return null;
    const h = host.trim();
    const us = user.trim();
    if (h.length === 0 || us.length === 0) return null;
    const parsed = Number(port.trim());
    const portNum =
      port.trim().length > 0 && Number.isInteger(parsed) && parsed > 0
        ? parsed
        : mode === 'implicit'
          ? 990
          : 21;
    const base = {
      kind: 'ftp' as const,
      host: h,
      port: portNum,
      user: us,
      path: ftpPath.trim(),
      mode,
    };
    return password.length > 0 ? { ...base, password } : base;
  }, [kind, path, url, host, port, ftpPath, mode, user, password]);

  const configure = useCallback(async () => {
    const config = buildConfig();
    if (config == null) return;
    setError(null);
    setBusy(true);
    try {
      await configureSyncAdapter(config);
      await refresh();
      announce(t('mobile.syncConfigured'));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, buildConfig, refresh, t]);

  // A stale preview would mislead — the join panel renders entirely from the
  // probed `preview`, but `joinTarget` joins the CURRENT `buildConfig()`. So
  // drop the preview whenever ANY field feeding the config changes (not just the
  // kind), forcing a re-check against the new target; announce it so the blind
  // user learns the previous result no longer applies. The backend re-derives
  // anyway, but this keeps the panel + the join atomic w.r.t. the live config.
  useEffect(() => {
    if (previewRef.current != null) {
      announce(t('mobile.syncPreviewStale'));
    }
    setPreview(null);
    setJoinPassphrase('');
    setJoinDeviceName('');
  }, [kind, path, url, host, port, ftpPath, mode, user, password, announce, t]);

  // Move SR focus onto the join panel when it appears (the §19.11 onboarding
  // twin of the trust-panel focus handling) so the blind user lands on the new
  // controls rather than hunting downward from the Check button.
  useEffect(() => {
    if (preview?.kind !== 'existing') return;
    const tag = joinPanelRef.current ? findNodeHandle(joinPanelRef.current) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [preview]);

  // Render the known-devices list for the §19.11 preview, marking this device.
  const summariseDevices = useCallback(
    (devices: SyncDeviceSummary[]): string =>
      devices
        .map((d) => {
          const base = d.name != null && d.name.length > 0 ? d.name : d.id;
          return d.is_this_device
            ? `${base} (${t('dialogs.settings.sync.previewThisDevice')})`
            : base;
        })
        .join(', '),
    [t],
  );

  // Probe the entered target WITHOUT committing (§19.11): Empty → "start fresh"
  // is the only path; Existing → reveal the join panel (passphrase if encrypted).
  const checkTarget = useCallback(async () => {
    const config = buildConfig();
    if (config == null) {
      setError(t('mobile.syncFieldsRequired'));
      announce(t('mobile.syncFieldsRequired'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const result = await previewSyncTarget(config);
      setPreview(result);
      announce(
        result.kind === 'empty'
          ? t('dialogs.settings.sync.previewEmpty')
          : t('dialogs.settings.sync.previewDevices', {
              count: result.devices.length,
              names: summariseDevices(result.devices),
            }),
      );
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, buildConfig, summariseDevices, t]);

  // Join the existing dataset (§19.11 "Adopt dataset"): pull + apply its
  // snapshot + logs and register this device. Non-destructive to the remote —
  // unlike "start fresh", which overwrites. A passphrase is required when the
  // dataset is encrypted (it's how this fresh device derives the key).
  const joinTarget = useCallback(async () => {
    const config = buildConfig();
    if (config == null || preview?.kind !== 'existing') return;
    if (preview.e2e_enabled && joinPassphrase.trim().length === 0) {
      setError(t('dialogs.settings.sync.e2ePassphraseRequired'));
      announce(t('dialogs.settings.sync.e2ePassphraseRequired'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const report = await acceptRemoteDataset(
        config,
        joinDeviceName.trim().length > 0 ? joinDeviceName.trim() : null,
        preview.e2e_enabled ? joinPassphrase : null,
      );
      setPreview(null);
      setJoinPassphrase('');
      await refresh();
      announce(
        t('dialogs.settings.sync.onboardingDone', { count: report.device_count }),
      );
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, buildConfig, joinDeviceName, joinPassphrase, preview, refresh, t]);

  // Dropbox / Google Drive: sign in via the browser (begin → native auth session
  // → store the refresh token), then activate the target in one step.
  const connectOauthTarget = useCallback(async () => {
    if (kind !== 'dropbox' && kind !== 'googledrive') return;
    const id = oauthClientId.trim();
    const secret = oauthClientSecret.trim();
    if (id.length === 0) {
      setError(t('dialogs.accounts.clientIdRequired'));
      announce(t('dialogs.accounts.clientIdRequired'));
      return;
    }
    if (kind === 'googledrive' && secret.length === 0) {
      setError(t('dialogs.accounts.clientSecretRequired'));
      announce(t('dialogs.accounts.clientSecretRequired'));
      return;
    }
    setError(null);
    setBusy(true);
    announce(t('mobile.oauthConnecting'));
    try {
      const result = await connectSyncOAuth({
        provider: kind,
        clientId: id,
        clientSecret: secret.length > 0 ? secret : undefined,
      });
      if (result.kind === 'cancelled') {
        announce(t('mobile.oauthCancelled'));
        return;
      }
      // Signed in (refresh token stored) → activate the target.
      const config: SyncAdapterConfig =
        kind === 'dropbox'
          ? {
              kind: 'dropbox',
              client_id: id,
              client_secret: secret.length > 0 ? secret : undefined,
              path: dropboxPath.trim() || undefined,
            }
          : {
              kind: 'googledrive',
              client_id: id,
              client_secret: secret,
              folder_name: folderName.trim() || undefined,
            };
      await configureSyncAdapter(config);
      await refresh();
      announce(t('mobile.syncConfigured'));
    } catch (err) {
      const raw = errorMessage(err);
      const message = raw === 'OAUTH_NO_CODE' ? t('mobile.oauthNoCode') : raw;
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [
    announce,
    dropboxPath,
    folderName,
    kind,
    oauthClientId,
    oauthClientSecret,
    refresh,
    t,
  ]);

  // Build the SFTP wire config from the fields, or null when a required field
  // (host/user/path, or key_path under key auth) is blank — the connect button
  // is then a no-op, matching the other kinds' "Use this target".
  const buildSftpConfig = useCallback((): Extract<
    SyncAdapterConfig,
    { kind: 'sftp' }
  > | null => {
    const host = sftpHost.trim();
    const user = sftpUser.trim();
    const path = sftpPath.trim();
    if (host.length === 0 || user.length === 0 || path.length === 0) return null;
    const base = {
      kind: 'sftp' as const,
      host,
      port: sftpPortNum,
      user,
      path,
      auth_method: sftpAuth,
    };
    if (sftpAuth === 'key') {
      const keyPath = sftpKeyPath.trim();
      if (keyPath.length === 0) return null;
      return sftpKeyPassphrase.length > 0
        ? { ...base, key_path: keyPath, key_passphrase: sftpKeyPassphrase }
        : { ...base, key_path: keyPath };
    }
    return sftpPassword.length > 0 ? { ...base, password: sftpPassword } : base;
  }, [
    sftpHost,
    sftpUser,
    sftpPath,
    sftpPortNum,
    sftpAuth,
    sftpKeyPath,
    sftpKeyPassphrase,
    sftpPassword,
  ]);

  // Activate the SFTP target (the pin is already recorded). Throws on a configure
  // failure so the caller surfaces it.
  const activateSftp = useCallback(async () => {
    const config = buildSftpConfig();
    if (config == null) return;
    await configureSyncAdapter(config);
    await refresh();
    announce(t('mobile.syncConfigured'));
  }, [announce, buildSftpConfig, refresh, t]);

  // SFTP connect: probe the host key FIRST (§19.5). Unchanged pin → connect
  // straight away; new/changed → surface the trust panel for explicit accept.
  const connectSftp = useCallback(async () => {
    const config = buildSftpConfig();
    if (config == null) {
      // A blind user pressing the only action button needs feedback, not a
      // silent no-op — say which fields are missing.
      setError(t('mobile.sftpFieldsRequired'));
      announce(t('mobile.sftpFieldsRequired'));
      return;
    }
    setError(null);
    setPendingTrust(null);
    setBusy(true);
    try {
      const preview = await previewSftpHostKey(config.host, config.port ?? 22);
      if (preview.status.kind === 'unchanged') {
        await activateSftp();
      } else {
        setPendingTrust(preview);
        const title =
          preview.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustChangedTitle')
            : t('dialogs.settings.sync.sftpTrustNewTitle');
        announce(
          `${title} ${t('dialogs.settings.sync.sftpTrustPresentedLabel')}: ${preview.fingerprint}`,
        );
      }
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [activateSftp, announce, buildSftpConfig, t]);

  // The user accepted the fingerprint in the trust panel: pin it, then connect.
  const confirmTrust = useCallback(async () => {
    if (pendingTrust == null) return;
    setError(null);
    setBusy(true);
    try {
      await trustSftpHostKey(pendingTrust.host_port, pendingTrust.fingerprint);
      setPendingTrust(null);
      // The focused trust panel just unmounted — return SR focus to the connect
      // button so the user isn't stranded after the security decision.
      focusSftpConnect();
      await activateSftp();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [activateSftp, announce, focusSftpConnect, pendingTrust, t]);

  const cancelTrust = useCallback(() => {
    setPendingTrust(null);
    focusSftpConnect();
    announce(t('mobile.sftpTrustCancelled'));
  }, [announce, focusSftpConnect, t]);

  // Drop the pinned fingerprint for the entered host (the next connect re-runs
  // the trust dialog). Useful when the user knows the server's key rotated.
  const forgetPin = useCallback(async () => {
    const host = sftpHost.trim();
    if (host.length === 0) return;
    try {
      await forgetSftpHostKey(`${host}:${sftpPortNum}`);
      announce(t('dialogs.settings.sync.sftpForgetPinDone'));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    }
  }, [announce, sftpHost, sftpPortNum, t]);

  const runSync = useCallback(async () => {
    setError(null);
    setBusy(true);
    announce(t('mobile.syncing'));
    try {
      const report = await syncNow();
      await refresh();
      announce(
        t('mobile.syncDone', {
          applied: report.applied,
          pushed: report.pushed_logs,
          fetched: report.fetched_logs,
        }),
      );
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, refresh, t]);

  // The async work behind enabling E2E (§19.7): mint + store the device-local
  // key and encrypt from here on. Split out from the confirmation gate so the
  // irreversible step only runs after the user confirms the warning dialog.
  const runEnableE2e = useCallback(
    async (pp: string) => {
      setError(null);
      setBusy(true);
      try {
        await enableSyncEncryption(pp);
        setE2ePassphrase('');
        await refresh();
        announce(t('dialogs.settings.sync.e2eActive'));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
      }
    },
    [announce, refresh, t],
  );

  // Enable E2E on the configured target — irreversible without the passphrase,
  // so we gate the mint behind an explicit confirmation (the Alert is
  // screen-reader-accessible and moves focus to itself, restating the warning).
  const enableE2e = useCallback(() => {
    const pp = e2ePassphrase.trim();
    if (pp.length === 0) {
      setError(t('dialogs.settings.sync.e2ePassphraseRequired'));
      announce(t('dialogs.settings.sync.e2ePassphraseRequired'));
      return;
    }
    Alert.alert(
      t('dialogs.settings.sync.e2eEnableLabel'),
      t('dialogs.settings.sync.e2eIrreversibleWarning'),
      [
        { text: t('dialogs.confirm.cancel'), style: 'cancel' },
        {
          text: t('dialogs.settings.sync.e2eEnableConfirm'),
          style: 'destructive',
          onPress: () => void runEnableE2e(pp),
        },
      ],
    );
  }, [announce, e2ePassphrase, runEnableE2e, t]);

  // Rotate the E2E passphrase (§19.7) — not destructive: the data key is
  // re-wrapped, not changed, so this device + every other already-onboarded one
  // keep working; only future joins use the new passphrase. No confirmation gate
  // (nothing is lost), matching the desktop.
  const changePassphrase = useCallback(async () => {
    const oldP = changeOldPp.trim();
    const newP = changeNewPp.trim();
    if (oldP.length === 0 || newP.length === 0) {
      setError(t('dialogs.settings.sync.passphraseChangeErrorEmpty'));
      announce(t('dialogs.settings.sync.passphraseChangeErrorEmpty'));
      return;
    }
    if (oldP === newP) {
      setError(t('dialogs.settings.sync.passphraseChangeErrorSame'));
      announce(t('dialogs.settings.sync.passphraseChangeErrorSame'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await changeSyncPassphrase(changeOldPp, changeNewPp);
      setChangeOldPp('');
      setChangeNewPp('');
      announce(t('dialogs.settings.sync.passphraseChangeOk'));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, changeNewPp, changeOldPp, t]);

  // The async half of disabling E2E — rewrites the whole dataset as plaintext.
  const runDisableE2e = useCallback(
    async (pp: string) => {
      setError(null);
      setBusy(true);
      try {
        const report = await disableSyncEncryption(pp);
        setDisablePp('');
        announce(
          t('dialogs.settings.sync.disableE2eOkAnnouncement', {
            logs: report.logs_rewritten,
          }),
        );
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusy(false);
        await refresh();
      }
    },
    [announce, refresh, t],
  );

  // Disable E2E — destructive for the cluster (every OTHER device must
  // re-onboard), so gate it behind a confirmation. Reuses the change-passphrase
  // "current passphrase" field as the verification input (desktop parity).
  const disableE2e = useCallback(() => {
    const pp = disablePp.trim();
    if (pp.length === 0) {
      setError(t('dialogs.settings.sync.disableE2eErrorNeedsPassphrase'));
      announce(t('dialogs.settings.sync.disableE2eErrorNeedsPassphrase'));
      return;
    }
    Alert.alert(
      t('dialogs.settings.sync.disableE2eAction'),
      t('dialogs.settings.sync.disableE2eConfirm'),
      [
        { text: t('dialogs.confirm.cancel'), style: 'cancel' },
        {
          text: t('dialogs.settings.sync.disableE2eAction'),
          style: 'destructive',
          onPress: () => void runDisableE2e(pp),
        },
      ],
    );
  }, [announce, disablePp, runDisableE2e, t]);

  // Adopt encryption a peer turned on (§19.7): a round failed with
  // `encryption_required`; the user supplies the dataset passphrase, we unlock +
  // switch to encrypted mode, then run a round (now decryptable) and refresh —
  // which clears the latch and removes this banner.
  const adoptEncryption = useCallback(async () => {
    const pp = adoptPp.trim();
    if (pp.length === 0) {
      setError(t('dialogs.settings.sync.e2ePassphraseRequired'));
      announce(t('dialogs.settings.sync.e2ePassphraseRequired'));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await adoptRemoteEncryption(adoptPp);
      setAdoptPp('');
      announce(t('dialogs.settings.sync.adoptRemoteE2eOk'));
      // Now that we can decrypt, run a round to pull the dataset.
      await syncNow();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
      // Always re-read status, even if the post-adopt round threw: the adopt
      // itself already succeeded (E2E is on), so the banner — gated on the
      // latched code — must reflect the NEW truth (a transient round error
      // overwrote the latch) and drop, rather than re-prompting for a
      // passphrase the user already entered. refresh swallows its own errors.
      await refresh();
    }
  }, [adoptPp, announce, refresh, t]);

  // Move SR focus onto the adopt banner when it appears (a round just failed
  // with encryption_required), so the blind user lands on the passphrase prompt.
  const adoptRequired = status?.last_error_code === 'encryption_required';
  useEffect(() => {
    if (!adoptRequired) return;
    const tag = adoptBannerRef.current
      ? findNodeHandle(adoptBannerRef.current)
      : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [adoptRequired]);

  const lastSynced =
    status?.last_synced_at != null
      ? new Date(status.last_synced_at).toLocaleString()
      : t('mobile.syncNever');

  // OAuth sign-in button labels (only used when isOAuthKind; cheap t() calls).
  const oauthSignInLabel =
    kind === 'dropbox'
      ? t('dialogs.settings.sync.adapterDropboxSignIn')
      : t('dialogs.settings.sync.adapterGoogledriveSignIn');
  const oauthSigningInLabel =
    kind === 'dropbox'
      ? t('dialogs.settings.sync.adapterDropboxSigningIn')
      : t('dialogs.settings.sync.adapterGoogledriveSigningIn');

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <Text
        style={styles.status}
        accessibilityRole="text"
        accessibilityLiveRegion="polite"
      >
        {status?.configured
          ? `${t('mobile.syncStatusConfigured')} ${t('mobile.syncLastSynced', { when: lastSynced })}`
          : t('mobile.syncStatusNotConfigured')}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {status?.sustained_failure === true && (
        <Text
          style={styles.warning}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {t('mobile.syncSustainedFailure')}
        </Text>
      )}

      {/* §19.9 — the dataset was written by a newer Aperio; this build can't
          apply it. A blocking notice (the running-dataset twin of the desktop
          SyncSchemaTooOldDialog) telling the user to update. */}
      {status?.schema_too_old === true && (
        <View style={styles.field}>
          <Text style={styles.label} accessibilityRole="header">
            {t('syncStatus.schemaTooOld')}
          </Text>
          <Text
            style={styles.warning}
            accessibilityRole="text"
            accessibilityLiveRegion="assertive"
          >
            {status.min_app_version_required != null
              ? `${t('syncStatus.announceSchemaTooOld')} (${status.min_app_version_required})`
              : t('syncStatus.announceSchemaTooOld')}
          </Text>
        </View>
      )}

      {/* §19.7 — another device turned encryption on; a round failed with
          encryption_required. Prompt for the dataset passphrase to adopt it. */}
      {adoptRequired && (
        <View style={styles.field}>
          <Text
            ref={adoptBannerRef}
            style={styles.label}
            accessibilityRole="header"
          >
            {t('dialogs.settings.sync.adoptRemoteE2eTitle')}
          </Text>
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.settings.sync.adoptRemoteE2eHint')}
          </Text>
          <Text style={styles.label}>
            {t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
          </Text>
          <TextInput
            style={styles.input}
            value={adoptPp}
            onChangeText={setAdoptPp}
            accessibilityLabel={t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
            secureTextEntry
            autoCapitalize="none"
            autoCorrect={false}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy, busy }}
            accessibilityLabel={t('dialogs.settings.sync.adoptRemoteE2eAction')}
            disabled={busy}
            onPress={() => void adoptEncryption()}
            style={({ pressed }) => [
              styles.primaryButton,
              pressed && styles.primaryPressed,
              busy && styles.primaryDisabled,
            ]}
          >
            <Text style={styles.primaryButtonText}>
              {busy
                ? t('dialogs.settings.sync.adoptRemoteE2eRunning')
                : t('dialogs.settings.sync.adoptRemoteE2eAction')}
            </Text>
          </Pressable>
        </View>
      )}

      {status?.configured && (
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={t('mobile.syncNow')}
          disabled={busy}
          onPress={() => void runSync()}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.primaryPressed,
            busy && styles.primaryDisabled,
          ]}
        >
          <Text style={styles.primaryButtonText}>
            {busy ? t('mobile.syncing') : t('mobile.syncNow')}
          </Text>
        </Pressable>
      )}

      {/* Unresolved sync conflicts → the resolution screen. Shown only when
          there are any; the count is announced via the polite live region. */}
      {conflictCount > 0 && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('syncStatus.openConflicts')}, ${t('syncStatus.conflict', {
            count: conflictCount,
          })}`}
          accessibilityLiveRegion="polite"
          onPress={() => navigation.navigate('Conflicts')}
          style={({ pressed }) => [styles.conflictsButton, pressed && styles.pressed]}
        >
          <Text style={styles.conflictsButtonText}>
            {t('syncStatus.openConflicts')} ({conflictCount})
          </Text>
        </Pressable>
      )}

      {/* End-to-end encryption (§19.7) — only meaningful once a target is set. */}
      {status?.configured &&
        (status.e2e_enabled ? (
          <>
            <Text
              style={styles.status}
              accessibilityRole="text"
              accessibilityLiveRegion="polite"
            >
              {t('dialogs.settings.sync.e2eActive')}
            </Text>
            {/* Rotate the passphrase — data unchanged; future joins use the new
                one. Not destructive, so no confirmation gate. */}
            <View style={styles.field}>
              <Text style={styles.label} accessibilityRole="header">
                {t('dialogs.settings.sync.passphraseChangeTitle')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.passphraseChangeHint')}
              </Text>
              <Text style={styles.label}>
                {t('dialogs.settings.sync.passphraseChangeOld')}
              </Text>
              <TextInput
                style={styles.input}
                value={changeOldPp}
                onChangeText={setChangeOldPp}
                accessibilityLabel={t('dialogs.settings.sync.passphraseChangeOld')}
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
              />
              <Text style={styles.label}>
                {t('dialogs.settings.sync.passphraseChangeNew')}
              </Text>
              <TextInput
                style={styles.input}
                value={changeNewPp}
                onChangeText={setChangeNewPp}
                accessibilityLabel={t('dialogs.settings.sync.passphraseChangeNew')}
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.settings.sync.passphraseChangeAction')}
                disabled={busy}
                onPress={() => void changePassphrase()}
                style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
              >
                <Text style={styles.ghostButtonText}>
                  {busy
                    ? t('dialogs.settings.sync.passphraseChangeRunning')
                    : t('dialogs.settings.sync.passphraseChangeAction')}
                </Text>
              </Pressable>
            </View>

            {/* Disable E2E — rewrites the dataset as plaintext. Cluster-
                destructive, so it has its OWN passphrase field (a blind user
                navigating linearly shouldn't have to discover that a field in
                the change-passphrase section above gates this action) and a
                confirmation in the handler. */}
            <View style={styles.field}>
              <Text style={styles.label} accessibilityRole="header">
                {t('dialogs.settings.sync.disableE2eAction')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.disableE2eHint')}
              </Text>
              <Text style={styles.label}>
                {t('dialogs.settings.sync.passphraseChangeOld')}
              </Text>
              <TextInput
                style={styles.input}
                value={disablePp}
                onChangeText={setDisablePp}
                accessibilityLabel={t('dialogs.settings.sync.passphraseChangeOld')}
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
              />
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.settings.sync.disableE2eAction')}
                disabled={busy}
                onPress={disableE2e}
                style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
              >
                <Text style={styles.ghostButtonText}>
                  {busy
                    ? t('dialogs.settings.sync.disableE2eRunning')
                    : t('dialogs.settings.sync.disableE2eAction')}
                </Text>
              </Pressable>
            </View>
          </>
        ) : (
          <View style={styles.field}>
            <Text style={styles.label} accessibilityRole="header">
              {t('dialogs.settings.sync.e2eEnableLabel')}
            </Text>
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.sync.e2eEnableHint')}
            </Text>
            <Text style={styles.warning} accessibilityRole="text">
              {t('dialogs.settings.sync.e2eIrreversibleWarning')}
            </Text>
            <Text style={styles.label}>
              {t('dialogs.settings.sync.e2ePassphrase')}
            </Text>
            <TextInput
              style={styles.input}
              value={e2ePassphrase}
              onChangeText={setE2ePassphrase}
              accessibilityLabel={t('dialogs.settings.sync.e2ePassphrase')}
              secureTextEntry
              autoCapitalize="none"
              autoCorrect={false}
            />
            <Pressable
              accessibilityRole="button"
              accessibilityState={{ disabled: busy }}
              accessibilityLabel={t('dialogs.settings.sync.e2eEnableLabel')}
              disabled={busy}
              onPress={enableE2e}
              style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
            >
              <Text style={styles.ghostButtonText}>
                {t('dialogs.settings.sync.e2eEnableLabel')}
              </Text>
            </Pressable>
          </View>
        ))}

      <RadioGroup<SyncKind>
        label={t('dialogs.settings.sync.adapterKind')}
        value={kind}
        options={kindOptions}
        onChange={setKind}
        disabled={busy}
      />

      {kind === 'local' && (
        <View style={styles.field}>
          <Text style={styles.label}>{t('mobile.syncPathLabel')}</Text>
          <Text style={styles.hint} accessibilityRole="text">
            {t('mobile.syncPathHint')}
          </Text>
          <TextInput
            style={styles.input}
            value={path}
            onChangeText={setPath}
            accessibilityLabel={t('mobile.syncPathLabel')}
            autoCapitalize="none"
            autoCorrect={false}
          />
        </View>
      )}

      {kind === 'webdav' && (
        <>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterWebdavUrl')}</Text>
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.sync.adapterWebdavUrlHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={url}
              onChangeText={setUrl}
              accessibilityLabel={t('dialogs.settings.sync.adapterWebdavUrl')}
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType="url"
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterWebdavUser')}</Text>
            <TextInput
              style={styles.input}
              value={user}
              onChangeText={setUser}
              accessibilityLabel={t('dialogs.settings.sync.adapterWebdavUser')}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterWebdavPassword')}</Text>
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.sync.adapterWebdavPasswordHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={password}
              onChangeText={setPassword}
              accessibilityLabel={t('dialogs.settings.sync.adapterWebdavPassword')}
              autoCapitalize="none"
              autoCorrect={false}
              secureTextEntry
            />
          </View>
        </>
      )}

      {kind === 'ftp' && (
        <>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterFtpHost')}</Text>
            <TextInput
              style={styles.input}
              value={host}
              onChangeText={setHost}
              accessibilityLabel={t('dialogs.settings.sync.adapterFtpHost')}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterFtpPort')}</Text>
            <TextInput
              style={styles.input}
              value={port}
              onChangeText={setPort}
              accessibilityLabel={t('dialogs.settings.sync.adapterFtpPort')}
              keyboardType="number-pad"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterFtpUser')}</Text>
            <TextInput
              style={styles.input}
              value={user}
              onChangeText={setUser}
              accessibilityLabel={t('dialogs.settings.sync.adapterFtpUser')}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterFtpPassword')}</Text>
            <TextInput
              style={styles.input}
              value={password}
              onChangeText={setPassword}
              accessibilityLabel={t('dialogs.settings.sync.adapterFtpPassword')}
              autoCapitalize="none"
              autoCorrect={false}
              secureTextEntry
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterFtpPath')}</Text>
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.sync.adapterFtpPathHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={ftpPath}
              onChangeText={setFtpPath}
              accessibilityLabel={t('dialogs.settings.sync.adapterFtpPath')}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <RadioGroup<FtpMode>
            label={t('dialogs.settings.sync.adapterFtpMode')}
            value={mode}
            options={ftpModeOptions}
            onChange={setMode}
            disabled={busy}
          />
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.settings.sync.adapterFtpModeHint')}
          </Text>
        </>
      )}

      {isOAuthKind && (
        <>
          <Text style={styles.hint} accessibilityRole="text">
            {kind === 'dropbox'
              ? t('dialogs.settings.sync.adapterDropboxIntro')
              : t('dialogs.settings.sync.adapterGoogledriveIntro')}
          </Text>
          <View style={styles.field}>
            <Text style={styles.label}>
              {kind === 'dropbox'
                ? t('dialogs.settings.sync.adapterDropboxClientId')
                : t('dialogs.settings.sync.adapterGoogledriveClientId')}
            </Text>
            <Text style={styles.hint} accessibilityRole="text">
              {kind === 'dropbox'
                ? t('dialogs.settings.sync.adapterDropboxClientIdHint')
                : t('dialogs.settings.sync.adapterGoogledriveClientIdHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={oauthClientId}
              onChangeText={setOauthClientId}
              accessibilityLabel={
                kind === 'dropbox'
                  ? t('dialogs.settings.sync.adapterDropboxClientId')
                  : t('dialogs.settings.sync.adapterGoogledriveClientId')
              }
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>
              {kind === 'dropbox'
                ? t('dialogs.settings.sync.adapterDropboxClientSecret')
                : t('dialogs.settings.sync.adapterGoogledriveClientSecret')}
            </Text>
            <Text style={styles.hint} accessibilityRole="text">
              {kind === 'dropbox'
                ? t('dialogs.settings.sync.adapterDropboxClientSecretHint')
                : t('dialogs.settings.sync.adapterGoogledriveClientSecretHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={oauthClientSecret}
              onChangeText={setOauthClientSecret}
              accessibilityLabel={
                kind === 'dropbox'
                  ? t('dialogs.settings.sync.adapterDropboxClientSecret')
                  : t('dialogs.settings.sync.adapterGoogledriveClientSecret')
              }
              secureTextEntry
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          {kind === 'dropbox' ? (
            <View style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.settings.sync.adapterDropboxPath')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.adapterDropboxPathHint')}
              </Text>
              <TextInput
                style={styles.input}
                value={dropboxPath}
                onChangeText={setDropboxPath}
                accessibilityLabel={t('dialogs.settings.sync.adapterDropboxPath')}
                autoCapitalize="none"
                autoCorrect={false}
              />
            </View>
          ) : (
            <View style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.settings.sync.adapterGoogledriveFolderName')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.adapterGoogledriveFolderNameHint')}
              </Text>
              <TextInput
                style={styles.input}
                value={folderName}
                onChangeText={setFolderName}
                accessibilityLabel={t(
                  'dialogs.settings.sync.adapterGoogledriveFolderName',
                )}
                autoCorrect={false}
              />
            </View>
          )}
        </>
      )}

      {isSftp && (
        <>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterSftpHost')}</Text>
            <TextInput
              style={styles.input}
              value={sftpHost}
              onChangeText={setSftpHost}
              accessibilityLabel={t('dialogs.settings.sync.adapterSftpHost')}
              editable={pendingTrust == null}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterSftpPort')}</Text>
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.sync.adapterSftpPortHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={sftpPort}
              onChangeText={setSftpPort}
              accessibilityLabel={t('dialogs.settings.sync.adapterSftpPort')}
              editable={pendingTrust == null}
              keyboardType="number-pad"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterSftpUser')}</Text>
            <TextInput
              style={styles.input}
              value={sftpUser}
              onChangeText={setSftpUser}
              accessibilityLabel={t('dialogs.settings.sync.adapterSftpUser')}
              editable={pendingTrust == null}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <View style={styles.field}>
            <Text style={styles.label}>{t('dialogs.settings.sync.adapterSftpPath')}</Text>
            <Text style={styles.hint} accessibilityRole="text">
              {t('dialogs.settings.sync.adapterSftpPathHint')}
            </Text>
            <TextInput
              style={styles.input}
              value={sftpPath}
              onChangeText={setSftpPath}
              accessibilityLabel={t('dialogs.settings.sync.adapterSftpPath')}
              editable={pendingTrust == null}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </View>
          <RadioGroup<SftpAuth>
            label={t('dialogs.settings.sync.adapterSftpAuthMethod')}
            value={sftpAuth}
            options={sftpAuthOptions}
            onChange={setSftpAuth}
            disabled={busy}
          />
          {sftpAuth === 'password' ? (
            <View style={styles.field}>
              <Text style={styles.label}>
                {t('dialogs.settings.sync.adapterSftpPassword')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.adapterSftpPasswordHint')}
              </Text>
              <TextInput
                style={styles.input}
                value={sftpPassword}
                onChangeText={setSftpPassword}
                accessibilityLabel={t('dialogs.settings.sync.adapterSftpPassword')}
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
              />
            </View>
          ) : (
            <>
              <View style={styles.field}>
                <Text style={styles.label}>
                  {t('dialogs.settings.sync.adapterSftpKeyPath')}
                </Text>
                <Text style={styles.hint} accessibilityRole="text">
                  {t('dialogs.settings.sync.adapterSftpKeyPathHint')}
                </Text>
                <TextInput
                  style={styles.input}
                  value={sftpKeyPath}
                  onChangeText={setSftpKeyPath}
                  accessibilityLabel={t('dialogs.settings.sync.adapterSftpKeyPath')}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>
              <View style={styles.field}>
                <Text style={styles.label}>
                  {t('dialogs.settings.sync.adapterSftpKeyPassphrase')}
                </Text>
                <Text style={styles.hint} accessibilityRole="text">
                  {t('dialogs.settings.sync.adapterSftpKeyPassphraseHint')}
                </Text>
                <TextInput
                  style={styles.input}
                  value={sftpKeyPassphrase}
                  onChangeText={setSftpKeyPassphrase}
                  accessibilityLabel={t('dialogs.settings.sync.adapterSftpKeyPassphrase')}
                  secureTextEntry
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>
            </>
          )}
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.settings.sync.sftpPinHint')}
          </Text>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy }}
            accessibilityLabel={t('dialogs.settings.sync.sftpForgetPin')}
            disabled={busy}
            onPress={() => void forgetPin()}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.settings.sync.sftpForgetPin')}
            </Text>
          </Pressable>
        </>
      )}

      {pendingTrust != null && (
        // NOT `accessible` on the container — that would collapse the whole
        // subtree (incl. the Accept/Cancel buttons) into one node, leaving a
        // screen-reader user able to read the fingerprint but unable to act on
        // it. Each child stays its own a11y node; focus lands on the title.
        <View accessibilityLiveRegion="assertive" style={styles.trustPanel}>
          <Text ref={trustRef} style={styles.trustTitle} accessibilityRole="header">
            {pendingTrust.status.kind === 'changed'
              ? t('dialogs.settings.sync.sftpTrustChangedTitle')
              : t('dialogs.settings.sync.sftpTrustNewTitle')}
          </Text>
          <Text style={styles.trustBody}>
            {pendingTrust.status.kind === 'changed'
              ? t('dialogs.settings.sync.sftpTrustChangedBody')
              : t('dialogs.settings.sync.sftpTrustNewBody')}
          </Text>
          <Text style={styles.trustField}>
            {t('dialogs.settings.sync.sftpTrustHostLabel')}: {pendingTrust.host_port}
          </Text>
          {pendingTrust.status.kind === 'changed' && (
            <Text style={styles.trustField}>
              {t('dialogs.settings.sync.sftpTrustStoredLabel')}:{' '}
              {pendingTrust.status.stored}
            </Text>
          )}
          <Text style={styles.trustField}>
            {t('dialogs.settings.sync.sftpTrustPresentedLabel')}:{' '}
            {pendingTrust.fingerprint}
          </Text>
          <Text style={styles.hint}>
            {t('dialogs.settings.sync.sftpTrustVerifyHint')}
          </Text>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy, busy }}
            accessibilityLabel={
              pendingTrust.status.kind === 'changed'
                ? t('dialogs.settings.sync.sftpTrustAcceptChanged')
                : t('dialogs.settings.sync.sftpTrustAcceptNew')
            }
            disabled={busy}
            onPress={() => void confirmTrust()}
            style={({ pressed }) => [
              styles.primaryButton,
              pressed && styles.primaryPressed,
              busy && styles.primaryDisabled,
            ]}
          >
            <Text style={styles.primaryButtonText}>
              {pendingTrust.status.kind === 'changed'
                ? t('dialogs.settings.sync.sftpTrustAcceptChanged')
                : t('dialogs.settings.sync.sftpTrustAcceptNew')}
            </Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy }}
            accessibilityLabel={t('dialogs.settings.sync.sftpTrustCancel')}
            disabled={busy}
            onPress={cancelTrust}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.settings.sync.sftpTrustCancel')}
            </Text>
          </Pressable>
        </View>
      )}

      {isOAuthKind ? (
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy, busy }}
          accessibilityLabel={oauthSignInLabel}
          disabled={busy}
          onPress={() => void connectOauthTarget()}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.primaryPressed,
            busy && styles.primaryDisabled,
          ]}
        >
          <Text style={styles.primaryButtonText}>
            {busy ? oauthSigningInLabel : oauthSignInLabel}
          </Text>
        </Pressable>
      ) : isSftp ? (
        <Pressable
          ref={sftpConnectRef}
          accessibilityRole="button"
          accessibilityState={{ disabled: busy, busy }}
          accessibilityLabel={t('dialogs.settings.sync.adapterConfigure')}
          disabled={busy}
          onPress={() => void connectSftp()}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.primaryPressed,
            busy && styles.primaryDisabled,
          ]}
        >
          <Text style={styles.primaryButtonText}>
            {busy
              ? t('dialogs.settings.sync.adapterConnecting')
              : t('dialogs.settings.sync.adapterConfigure')}
          </Text>
        </Pressable>
      ) : (
        <>
          {/* §19.11 onboarding: probe the target before committing, so an
              existing (possibly encrypted) dataset can be JOINED rather than
              silently overwritten. */}
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy }}
            accessibilityLabel={t('dialogs.settings.sync.previewButton')}
            disabled={busy}
            onPress={() => void checkTarget()}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.settings.sync.previewButton')}
            </Text>
          </Pressable>

          {preview?.kind === 'empty' && (
            <Text
              style={styles.hint}
              accessibilityRole="text"
              accessibilityLiveRegion="polite"
            >
              {t('dialogs.settings.sync.previewEmpty')}
            </Text>
          )}

          {preview?.kind === 'existing' && (
            <View style={styles.field}>
              <Text ref={joinPanelRef} style={styles.label} accessibilityRole="header">
                {t('dialogs.settings.sync.previewAcceptTitle')}
              </Text>
              <Text
                style={styles.hint}
                accessibilityRole="text"
                accessibilityLiveRegion="polite"
              >
                {preview.snapshot_timestamp != null
                  ? t('dialogs.settings.sync.previewExisting', {
                      time: new Date(preview.snapshot_timestamp).toLocaleString(),
                    })
                  : t('dialogs.settings.sync.previewNeverCompacted')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.previewDevices', {
                  count: preview.devices.length,
                  names: summariseDevices(preview.devices),
                })}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.previewAcceptBody')}
              </Text>

              {preview.compatibility.kind === 'app_too_old' && (
                <Text style={styles.warning} accessibilityRole="text">
                  {t('dialogs.settings.sync.errorSchemaTooOld')}
                </Text>
              )}

              {preview.e2e_enabled && (
                <>
                  <Text style={styles.hint} accessibilityRole="text">
                    {t('dialogs.settings.sync.adoptRemoteE2eHint')}
                  </Text>
                  <Text style={styles.label}>
                    {t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
                  </Text>
                  <TextInput
                    style={styles.input}
                    value={joinPassphrase}
                    onChangeText={setJoinPassphrase}
                    accessibilityLabel={t(
                      'dialogs.settings.sync.adoptRemoteE2ePassphraseLabel',
                    )}
                    secureTextEntry
                    autoCapitalize="none"
                    autoCorrect={false}
                  />
                </>
              )}

              <Text style={styles.label}>
                {t('dialogs.settings.sync.deviceName')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.deviceNameHint')}
              </Text>
              <TextInput
                style={styles.input}
                value={joinDeviceName}
                onChangeText={setJoinDeviceName}
                accessibilityLabel={t('dialogs.settings.sync.deviceName')}
                autoCapitalize="words"
                autoCorrect={false}
              />

              <Pressable
                accessibilityRole="button"
                accessibilityState={{
                  disabled: busy || preview.compatibility.kind === 'app_too_old',
                }}
                accessibilityLabel={t('dialogs.settings.sync.previewAcceptButton')}
                disabled={busy || preview.compatibility.kind === 'app_too_old'}
                onPress={() => void joinTarget()}
                style={({ pressed }) => [
                  styles.primaryButton,
                  pressed && styles.primaryPressed,
                  (busy || preview.compatibility.kind === 'app_too_old') &&
                    styles.primaryDisabled,
                ]}
              >
                <Text style={styles.primaryButtonText}>
                  {t('dialogs.settings.sync.previewAcceptButton')}
                </Text>
              </Pressable>
            </View>
          )}

          {/* "Start fresh / overwrite": the original configure path. */}
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy }}
            accessibilityLabel={t('mobile.syncConfigure')}
            disabled={busy}
            onPress={() => void configure()}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>{t('mobile.syncConfigure')}</Text>
          </Pressable>
        </>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 16 },
  status: { fontSize: 16, color: '#10131a', fontWeight: '600' },
  field: { gap: 6 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  hint: { fontSize: 13, color: '#5b6573' },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  primaryButton: {
    paddingVertical: 14,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  primaryPressed: { backgroundColor: '#1740a8' },
  primaryDisabled: { backgroundColor: '#9aa9c9' },
  primaryButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  conflictsButton: {
    paddingVertical: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#f0c2bd',
    backgroundColor: '#fdecea',
    alignItems: 'center',
  },
  conflictsButtonText: { fontSize: 16, fontWeight: '700', color: '#b42318' },
  ghostButton: {
    paddingVertical: 12,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
    alignItems: 'center',
  },
  ghostButtonText: { fontSize: 16, fontWeight: '600', color: '#1d3a2f' },
  pressed: { opacity: 0.7 },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  warning: {
    fontSize: 15,
    fontWeight: '600',
    color: '#92400e',
    backgroundColor: '#fef3c7',
    padding: 12,
    borderRadius: 10,
  },
  trustPanel: {
    gap: 8,
    padding: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#d9b3b0',
    backgroundColor: '#fbeceb',
  },
  trustTitle: { fontSize: 17, fontWeight: '700', color: '#10131a' },
  trustBody: { fontSize: 14, color: '#2b3240' },
  // Monospace so the fingerprint reads character-by-character (and a SR user can
  // compare it exactly against the out-of-band value).
  trustField: { fontSize: 14, color: '#10131a', fontFamily: 'monospace' },
});
