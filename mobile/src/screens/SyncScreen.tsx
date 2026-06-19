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
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  acceptRemoteDataset,
  adoptLocalDataset,
  adoptRemoteEncryption,
  cacheRefreshStatus,
  changeSyncPassphrase,
  compactNow,
  configureSyncAdapter,
  disableSyncEncryption,
  enableSyncEncryption,
  forgetSftpHostKey,
  previewSftpHostKey,
  previewSyncTarget,
  refreshExternalCache,
  resumeStaleDevice,
  trustSftpHostKey,
  clearSyncLog,
  listSyncLog,
  syncConflictCount,
  syncNow,
  syncStatus,
  CacheRefreshStatus,
  HostKeyPreview,
  SyncAdapterConfig,
  SyncDeviceSummary,
  SyncLogEntry,
  SyncPreview,
  SyncStatus,
} from '../api/sync';
import { listAccounts } from '../api/accounts';
import { connectSyncOAuth } from '../api/oauth';
import { setUserPref } from '../api/prefs';
import { useScreenA11yInert } from '../a11y/useScreenA11yInert';
import { AppDialog } from '../components/AppDialog';
import { RadioGroup } from '../components/RadioGroup';
import { useThemedStyles, type ThemeColors } from '../theme';
import CalFfi from '../../modules/cal-ffi';

// Cross-device sync — a full desktop peer (same engine, statically-embedded
// adapters). This screen exposes the password targets (a local shared folder,
// WebDAV, FTPS), the OAuth targets (Dropbox, Google Drive — BYO client-id,
// browser sign-in), and SFTP (SSH server, with the §19.5 host-key TOFU trust
// flow). Screen-reader-first: the kind is a radio group, every field is its own
// labelled stop, the trust panel is an assertive live region, results announced.

type SyncKind = 'local' | 'webdav' | 'ftp' | 'dropbox' | 'googledrive' | 'sftp';
type FtpMode = 'explicit' | 'implicit' | 'plain';
type SftpAuth = 'password' | 'key';

// The synced sync-interval pref (same key the foreground periodic timer reads in
// syncTriggers) + the preset choices, mirroring the desktop SyncPanel. Writing
// the pref is all mobile needs: there's no persistent scheduler to kick — the
// foreground timer re-reads the pref on each resume.
const PREF_SYNC_INTERVAL_MINUTES = 'sync.intervalMinutes';
const INTERVAL_PRESETS: readonly number[] = [1, 5, 15, 30, 60, 240];

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SyncScreen() {
  const { t } = useTranslation();
  const navigation = useNavigation();
  const styles = useThemedStyles(makeStyles);
  // Leave the screen out of the a11y tree while it isn't focused so an
  // interrupted back-swipe / stack transition can't strand VoiceOver on a stale
  // node here (issue #1 — same guard SettingsScreen uses).
  const inert = useScreenA11yInert();

  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [conflictCount, setConflictCount] = useState(0);
  const [syncLog, setSyncLog] = useState<SyncLogEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [busyCompact, setBusyCompact] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The external-cache warm-pass status (the desktop's cache surface): drives
  // the "refreshing…" / "last updated" line + the manual refresh button. Loaded
  // on focus and updated live from the native `onCacheRefreshStatus` event.
  const [cacheStatus, setCacheStatus] = useState<CacheRefreshStatus | null>(null);
  // The external SWR cache (the "External data" section below) warms EXTERNAL
  // accounts (calendar/task/contact providers), independent of the sync target —
  // so that section is shown only when at least one external (non-local) account
  // exists; otherwise its "last synchronized" line misleads with nothing connected.
  const [hasExternalAccounts, setHasExternalAccounts] = useState(false);

  // Which target to configure, plus the per-kind fields. `user`/`password` are
  // shared by WebDAV + FTP (only the active kind's fields are shown).
  // 'local' is intentionally NOT offered on mobile (see kindOptions): a
  // device-local target only backs up into the app sandbox, which no other
  // device can sync against. The SyncKind union + the dormant `kind === 'local'`
  // branches stay so a 'local' config synced from desktop still round-trips.
  const [kind, setKind] = useState<SyncKind>('webdav');
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
  // Open while collecting the passphrase for an EXISTING ENCRYPTED dataset, in a
  // focus-trapping dialog — so the field is impossible to miss and the
  // destructive "start fresh" stays out of the join path (issues #2 / #4).
  const [joinDialogOpen, setJoinDialogOpen] = useState(false);
  const [joinDeviceName, setJoinDeviceName] = useState(''); // optional name for meta.json
  // The empty-target "enable E2E at creation" toggle — the unified Connect
  // button's second-press option; reuses joinPassphrase as the passphrase draft.
  const [createE2e, setCreateE2e] = useState(false);
  // Latest preview, readable inside the invalidation effect WITHOUT making
  // `preview` a dep (which would re-run + clear it in a loop). Lets the effect
  // announce "the check is now stale" only when one was actually showing.
  const previewRef = useRef<SyncPreview | null>(null);
  previewRef.current = preview;
  // The join-panel title — SR focus lands here when the panel appears (mirrors
  // the SFTP trust panel) so the blind user reaches the passphrase/Adopt controls
  // instead of being stranded on the "Check existing dataset" button.
  const joinPanelRef = useRef<Text>(null);
  const emptyPanelRef = useRef<Text>(null);
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

  // Change the foreground periodic-sync interval: persist the synced pref (the
  // timer re-reads it on the next resume) + re-read the status so the picker
  // reflects the clamped value. Mirrors the desktop set_sync_interval path.
  const onIntervalChange = useCallback(
    async (minutes: number) => {
      try {
        await setUserPref(PREF_SYNC_INTERVAL_MINUTES, String(minutes));
        setStatus(await syncStatus());
        announce(t('dialogs.settings.sync.intervalChanged', { minutes }));
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, t],
  );

  const intervalOptions = useMemo(
    () =>
      INTERVAL_PRESETS.map((min) => ({
        value: min,
        label: t('dialogs.settings.sync.intervalOption', { count: min, minutes: min }),
      })),
    [t],
  );

  // Manual compaction (§19.10): snapshot + GC old logs. Compaction also runs
  // automatically at the thresholds; this is the override. Refresh the protocol
  // afterwards so its recorded row shows.
  const onCompact = useCallback(async () => {
    setBusyCompact(true);
    try {
      const report = await compactNow();
      announce(
        t('dialogs.settings.sync.compactDone', { deleted: report.deleted_logs }),
      );
      setSyncLog(await listSyncLog(100).catch(() => []));
    } catch (err) {
      announce(t('mobile.error', { message: errorMessage(err) }));
    } finally {
      setBusyCompact(false);
    }
  }, [announce, t]);

  const refresh = useCallback(async () => {
    try {
      setStatus(await syncStatus());
      setConflictCount(await syncConflictCount().catch(() => 0));
      setSyncLog(await listSyncLog(100).catch(() => []));
      setCacheStatus(await cacheRefreshStatus().catch(() => null));
      setHasExternalAccounts(
        (await listAccounts().catch(() => [])).some((a) => a.adapter_kind !== 'local'),
      );
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

  // Subscribe to the native external-cache warm-pass status WHILE FOCUSED so the
  // "refreshing…" / "last updated" line follows a pass live (a manual refresh
  // here, or a background warm). Parse the event's JSON `CacheRefreshStatus`.
  // Removed on blur/unmount so the listener never leaks.
  useFocusEffect(
    useCallback(() => {
      const sub = CalFfi.addListener('onCacheRefreshStatus', ({ status: json }) => {
        try {
          setCacheStatus(JSON.parse(json) as CacheRefreshStatus);
        } catch {
          // A malformed payload just leaves the last-known status in place.
        }
      });
      return () => sub.remove();
    }, []),
  );

  // Kick an immediate warm pass over every external account (the manual "refresh
  // now"). Fire-and-forget: the native `onCacheRefreshStatus` subscription above
  // streams the refreshing→done transition into the status line.
  const refreshCache = useCallback(async () => {
    announce(t('cacheRefresh.refreshing'));
    try {
      await refreshExternalCache();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    }
  }, [announce, t]);

  // Clear the diagnostic sync log (a confirm — it scrubs only the local history,
  // never any sync data).
  const clearLog = useCallback(() => {
    Alert.alert(
      t('dialogs.settings.sync.protocolClear'),
      t('dialogs.settings.sync.protocolClearConfirm'),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.settings.sync.protocolClear'),
          style: 'destructive',
          onPress: () => {
            void (async () => {
              try {
                await clearSyncLog();
                await refresh();
                announce(t('dialogs.settings.sync.protocolCleared'));
              } catch (err) {
                const message = errorMessage(err);
                setError(message);
                announce(t('mobile.error', { message }));
              }
            })();
          },
        },
      ],
    );
  }, [announce, refresh, t]);

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

  // The unified §19.11 Connect — the mobile twin of the desktop `runConnect`.
  // Probe the target, then JOIN an existing dataset or INITIALISE an empty one,
  // replacing the old check/join/"use this folder" three-button dance. An empty
  // target takes TWO presses (the first reveals the optional E2E setup, the
  // second creates the dataset) so we never silently overwrite; an existing
  // encrypted dataset reveals its passphrase field on the first press and joins
  // on the next.
  const onConnect = useCallback(async () => {
    const config = buildConfig();
    if (config == null) {
      setError(t('mobile.syncFieldsRequired'));
      announce(t('mobile.syncFieldsRequired'));
      return;
    }
    const priorPreview = preview;
    const device = joinDeviceName.trim().length > 0 ? joinDeviceName.trim() : null;
    setError(null);
    setBusy(true);
    try {
      const p = await previewSyncTarget(config);
      setPreview(p);
      if (p.kind === 'existing') {
        // This build can't apply a newer dataset (§19.9) — surface the warning
        // (the panel also shows it) but never join.
        if (p.compatibility.kind === 'app_too_old') {
          setError(t('dialogs.settings.sync.errorSchemaTooOld'));
          announce(t('dialogs.settings.sync.errorSchemaTooOld'));
          return;
        }
        // Encrypted → collect the passphrase in a focus-trapping dialog and
        // join from there (the field is impossible to miss, and the destructive
        // "start fresh" stays out of the join path). The preview stays so the
        // panel still offers "start fresh" as a deliberate, separate choice.
        if (p.e2e_enabled) {
          setJoinPassphrase('');
          setJoinDialogOpen(true);
          announce(t('dialogs.settings.sync.e2eRemoteRequiresPassphrase'));
          return;
        }
        // Plaintext existing → join immediately (non-destructive).
        const report = await acceptRemoteDataset(config, device, null);
        setPreview(null);
        setJoinPassphrase('');
        setCreateE2e(false);
        await refresh();
        announce(
          t('dialogs.settings.sync.onboardingDone', { count: report.device_count }),
        );
        return;
      }
      // Empty target. The first press only REVEALS the optional E2E setup; only
      // a second press (priorPreview already 'empty') initialises the dataset.
      if (priorPreview?.kind !== 'empty') {
        announce(t('dialogs.settings.sync.connectEmptyReveal'));
        return;
      }
      if (createE2e && joinPassphrase.trim().length === 0) {
        setError(t('dialogs.settings.sync.e2ePassphraseRequired'));
        announce(t('dialogs.settings.sync.e2ePassphraseRequired'));
        return;
      }
      await adoptLocalDataset(config, device, createE2e ? joinPassphrase : null);
      setPreview(null);
      setJoinPassphrase('');
      setCreateE2e(false);
      await refresh();
      announce(t('dialogs.settings.sync.onboardingFresh'));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      setPreview(null);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [
    announce,
    buildConfig,
    createE2e,
    joinDeviceName,
    joinPassphrase,
    preview,
    refresh,
    t,
  ]);

  // Join the probed EXISTING ENCRYPTED dataset with the passphrase entered in
  // the dialog (the non-destructive adopt path). The Confirm is gated on a
  // non-empty passphrase, so this never reaches the bridge blank.
  const joinExisting = useCallback(async () => {
    const config = buildConfig();
    if (config == null || joinPassphrase.trim().length === 0) return;
    const device = joinDeviceName.trim().length > 0 ? joinDeviceName.trim() : null;
    setError(null);
    setBusy(true);
    try {
      const report = await acceptRemoteDataset(config, device, joinPassphrase);
      setJoinDialogOpen(false);
      setPreview(null);
      setJoinPassphrase('');
      setCreateE2e(false);
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
  }, [announce, buildConfig, joinDeviceName, joinPassphrase, refresh, t]);

  // Destructive "start fresh" over an EXISTING dataset — overwrites the remote
  // meta.json, orphaning other devices' sync. Demoted behind an Alert confirm
  // (the mobile twin of the desktop onOverwrite): a plaintext re-init
  // (passphrase null); the user can enable E2E afterwards.
  const confirmOverwrite = useCallback(() => {
    const config = buildConfig();
    if (config == null) return;
    const device = joinDeviceName.trim().length > 0 ? joinDeviceName.trim() : null;
    Alert.alert(
      t('dialogs.settings.sync.previewAdoptButton'),
      t('dialogs.settings.sync.previewAdoptConfirm'),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.settings.sync.previewAdoptButton'),
          style: 'destructive',
          onPress: () => {
            setError(null);
            setBusy(true);
            void (async () => {
              try {
                await adoptLocalDataset(config, device, null);
                setPreview(null);
                setJoinPassphrase('');
                setCreateE2e(false);
                await refresh();
                announce(t('dialogs.settings.sync.onboardingFresh'));
              } catch (err) {
                const message = errorMessage(err);
                setError(message);
                announce(t('mobile.error', { message }));
              } finally {
                setBusy(false);
              }
            })();
          },
        },
      ],
    );
  }, [announce, buildConfig, joinDeviceName, refresh, t]);

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
    setCreateE2e(false);
    // joinDeviceName is intentionally kept — it names THIS device, not the
    // target, so editing the connection fields shouldn't wipe it.
  }, [kind, path, url, host, port, ftpPath, mode, user, password, announce, t]);

  // Move SR focus onto the join panel when it appears (the §19.11 onboarding
  // twin of the trust-panel focus handling) so the blind user lands on the new
  // controls rather than hunting downward from the Check button.
  useEffect(() => {
    // The encrypted-existing case opens the passphrase dialog, which drives its
    // own focus — don't also yank focus onto the panel title behind it (a race
    // that contributed to the "focus jumps around" report).
    if (preview?.kind === 'existing' && preview.e2e_enabled) return;
    const ref =
      preview?.kind === 'existing'
        ? joinPanelRef
        : preview?.kind === 'empty'
          ? emptyPanelRef
          : null;
    if (ref == null) return;
    const tag = ref.current ? findNodeHandle(ref.current) : null;
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

  // §19.10 — this device fell so far behind the dataset got compacted; re-onboard
  // (full snapshot) + clear the stale flag. Local offline edits stay in the push
  // queue and go out next round.
  const resumeStale = useCallback(async () => {
    setError(null);
    setBusy(true);
    try {
      const report = await resumeStaleDevice();
      announce(t('syncStaleResume.doneAnnouncement', { applied: report.applied }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(`${t('syncStaleResume.errorPrefix')}: ${message}`);
    } finally {
      setBusy(false);
      await refresh();
    }
  }, [announce, refresh, t]);

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

  // The external-cache status line: refreshing now → "last updated …" → "never".
  const cacheStatusLine = cacheStatus?.refreshing
    ? t('cacheRefresh.refreshing')
    : cacheStatus?.last_refreshed_at != null
      ? t('cacheRefresh.lastUpdated', {
          time: new Date(cacheStatus.last_refreshed_at).toLocaleString(),
        })
      : t('cacheRefresh.never');

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
      {...inert}
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

      {status?.configured && status?.sustained_failure === true && (
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

      {/* §19.10 — this device went stale (offline past the compaction window).
          Offer a full re-onboard; local offline edits stay queued. */}
      {status?.stale_device_since != null && (
        <View style={styles.field}>
          <Text style={styles.label} accessibilityRole="header">
            {t('syncStaleResume.title')}
          </Text>
          <Text style={styles.hint} accessibilityRole="text">
            {t('syncStaleResume.mergeHint')}
          </Text>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy, busy }}
            accessibilityLabel={t('syncStaleResume.actionContinue')}
            disabled={busy}
            onPress={() => void resumeStale()}
            style={({ pressed }) => [
              styles.primaryButton,
              pressed && styles.primaryPressed,
              busy && styles.primaryDisabled,
            ]}
          >
            <Text style={styles.primaryButtonText}>
              {busy ? t('syncStaleResume.applying') : t('syncStaleResume.actionContinue')}
            </Text>
          </Pressable>
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

      {/* Foreground periodic-sync interval — only meaningful once a target is
          configured (the timer only runs against a real target). */}
      {status?.configured && (
        <View style={styles.field}>
          <RadioGroup<number>
            label={t('dialogs.settings.sync.intervalLabel')}
            value={status.interval_minutes}
            options={intervalOptions}
            onChange={(min) => void onIntervalChange(min)}
            disabled={busy}
          />
        </View>
      )}

      {/* Manual compaction (§19.10): snapshot + GC old logs. Auto-runs at the
          thresholds too; this is the override. Configured-only. */}
      {status?.configured && (
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busyCompact }}
          accessibilityLabel={t('dialogs.settings.sync.compactNow')}
          disabled={busyCompact}
          onPress={() => void onCompact()}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText}>
            {busyCompact
              ? t('dialogs.settings.sync.compacting')
              : t('dialogs.settings.sync.compactNow')}
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
          {/* §19.11 onboarding — a SINGLE "Connect" button: probe the target,
              then JOIN an existing dataset or INITIALISE an empty one (an empty
              target needs a second press to confirm, so we never silently
              overwrite). The device name (set before pressing) names THIS
              device on the dataset. */}
          <Text style={styles.label}>{t('dialogs.settings.sync.deviceName')}</Text>
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
            accessibilityState={{ disabled: busy, busy }}
            accessibilityLabel={t('dialogs.settings.sync.adapterConfigure')}
            disabled={busy}
            onPress={() => void onConnect()}
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

          {/* EMPTY target, revealed after the FIRST Connect press: optional
              end-to-end encryption. A SECOND Connect press creates the dataset. */}
          {preview?.kind === 'empty' && (
            <View style={styles.field}>
              <Text
                ref={emptyPanelRef}
                style={styles.label}
                accessibilityRole="header"
                accessibilityLiveRegion="polite"
              >
                {t('dialogs.settings.sync.connectEmptyReveal')}
              </Text>
              <Pressable
                accessibilityRole="switch"
                accessibilityState={{ checked: createE2e }}
                accessibilityLabel={t('dialogs.settings.sync.e2eEnableLabel')}
                onPress={() => setCreateE2e((prev) => !prev)}
                style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
              >
                <Text style={styles.switchLabel} importantForAccessibility="no">
                  {t('dialogs.settings.sync.e2eEnableLabel')}
                </Text>
                <View pointerEvents="none">
                  <Switch
                    value={createE2e}
                    importantForAccessibility="no"
                    accessibilityElementsHidden
                  />
                </View>
              </Pressable>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.e2eEnableHint')}
              </Text>
              {createE2e && (
                <>
                  <Text style={styles.label}>
                    {t('dialogs.settings.sync.e2ePassphrase')}
                  </Text>
                  <TextInput
                    style={styles.input}
                    value={joinPassphrase}
                    onChangeText={setJoinPassphrase}
                    accessibilityLabel={t('dialogs.settings.sync.e2ePassphrase')}
                    secureTextEntry
                    autoCapitalize="none"
                    autoCorrect={false}
                  />
                  <Text style={styles.warning} accessibilityRole="text">
                    {t('dialogs.settings.sync.e2eIrreversibleWarning')}
                  </Text>
                </>
              )}
            </View>
          )}

          {/* EXISTING target: the dataset details + passphrase (when encrypted).
              The join fires on the NEXT Connect press, not a separate button. */}
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

              {/* Encrypted: the non-destructive JOIN — opens the focus-trapping
                  passphrase dialog. Kept ABOVE the destructive overwrite so a
                  linear screen-reader pass reaches "Join" before "Start fresh"
                  (issue #4). */}
              {preview.e2e_enabled && (
                <Pressable
                  accessibilityRole="button"
                  accessibilityState={{ disabled: busy }}
                  accessibilityLabel={t('dialogs.settings.sync.previewJoinButton')}
                  disabled={busy}
                  onPress={() => {
                    setJoinPassphrase('');
                    setJoinDialogOpen(true);
                  }}
                  style={({ pressed }) => [
                    styles.primaryButton,
                    pressed && styles.primaryPressed,
                    busy && styles.primaryDisabled,
                  ]}
                >
                  <Text style={styles.primaryButtonText}>
                    {t('dialogs.settings.sync.previewJoinButton')}
                  </Text>
                </Pressable>
              )}

              {/* Destructive "start fresh" over THIS dataset — overwrites the
                  remote (other devices lose their sync), so it sits behind a
                  confirm and apart from the non-destructive join. */}
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.settings.sync.previewAdoptButton')}
                disabled={busy}
                onPress={confirmOverwrite}
                style={({ pressed }) => [
                  styles.conflictsButton,
                  pressed && styles.pressed,
                ]}
              >
                <Text style={styles.conflictsButtonText}>
                  {t('dialogs.settings.sync.previewAdoptButton')}
                </Text>
              </Pressable>
            </View>
          )}
        </>
      )}

      {/* External data — explicit controls over the stale-while-revalidate
          external cache (the desktop's cache surface): a manual "refresh now"
          plus a live status line. The list views (calendar/tasks/contacts)
          live-reload + announce on a warm pass via the root cache observer, so
          this section is the control point, not a duplicate announcer. */}
      {hasExternalAccounts && (
        <View style={styles.protocolSection}>
          <Text style={styles.label} accessibilityRole="header">
            {t('cacheRefresh.label')}
          </Text>
          <Text
            style={styles.hint}
            accessibilityRole="text"
            accessibilityLiveRegion="polite"
          >
            {cacheStatusLine}
          </Text>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: cacheStatus?.refreshing === true }}
            accessibilityLabel={t('cacheRefresh.refreshNow')}
            disabled={cacheStatus?.refreshing === true}
            onPress={() => void refreshCache()}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>{t('cacheRefresh.refreshNow')}</Text>
          </Pressable>
        </View>
      )}

      {/* Protocol — recent sync rounds (newest first), the diagnostic log every
          round self-records (mobile has no scheduler). A linear accessible list;
          each row's trigger + outcome + counts + time fold into one SR label. */}
      <View style={styles.protocolSection}>
        <Text style={styles.label} accessibilityRole="header">
          {t('dialogs.settings.sync.protocolTitle')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.sync.protocolBody')}
        </Text>
        {syncLog.length === 0 ? (
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.settings.sync.protocolEmpty')}
          </Text>
        ) : (
          <View
            accessibilityRole="list"
            accessibilityLabel={t('dialogs.settings.sync.protocolListLabel')}
            style={styles.protocolList}
          >
            {syncLog.map((entry) => {
              const triggerLabel = t(
                `dialogs.settings.sync.protocolTrigger${
                  entry.trigger === 'app_start'
                    ? 'AppStart'
                    : entry.trigger === 'app_exit'
                      ? 'AppExit'
                      : entry.trigger.charAt(0).toUpperCase() + entry.trigger.slice(1)
                }`,
                entry.trigger,
              );
              const summary = entry.success
                ? t('dialogs.settings.sync.protocolSummarySuccess', {
                    pushed: entry.pushed_logs ?? 0,
                    fetched: entry.fetched_logs ?? 0,
                    applied: entry.applied ?? 0,
                  })
                : t('dialogs.settings.sync.protocolSummaryFailure', {
                    error: entry.error ?? '',
                  });
              const when = new Date(entry.recorded_at).toLocaleString();
              const duration =
                entry.duration_ms != null
                  ? t('dialogs.settings.sync.protocolDuration', { ms: entry.duration_ms })
                  : '';
              return (
                <View
                  key={entry.id}
                  accessible
                  accessibilityRole="text"
                  accessibilityLabel={`${triggerLabel}, ${when}, ${summary}${duration ? `, ${duration}` : ''}`}
                  style={styles.protocolRow}
                >
                  <Text style={styles.protocolRowHead} importantForAccessibility="no">
                    {`${triggerLabel} · ${when}`}
                  </Text>
                  <Text
                    style={[
                      styles.protocolRowSummary,
                      !entry.success && styles.protocolRowError,
                    ]}
                    importantForAccessibility="no"
                  >
                    {summary}
                    {duration ? ` · ${duration}` : ''}
                  </Text>
                </View>
              );
            })}
          </View>
        )}
        {syncLog.length > 0 && (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.settings.sync.protocolClear')}
            onPress={clearLog}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.settings.sync.protocolClear')}
            </Text>
          </Pressable>
        )}
      </View>

      {/* Encrypted-dataset join: a focus-trapping popup that owns the passphrase
          entry. Confirm (Join) stays greyed until a passphrase is typed; Cancel
          / tap-outside leaves the dataset untouched (issues #2 / #4). */}
      <AppDialog
        visible={joinDialogOpen}
        title={t('dialogs.settings.sync.joinEncryptedTitle')}
        message={t('dialogs.settings.sync.e2eRemoteRequiresPassphrase')}
        input={{
          value: joinPassphrase,
          onChangeText: setJoinPassphrase,
          label: t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel'),
          secureTextEntry: true,
        }}
        confirmLabel={t('dialogs.settings.sync.previewJoinButton')}
        cancelLabel={t('mobile.cancel')}
        confirmDisabled={joinPassphrase.trim().length === 0}
        busy={busy}
        onConfirm={() => void joinExisting()}
        onCancel={() => {
          setJoinDialogOpen(false);
          setJoinPassphrase('');
        }}
      />
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    status: { fontSize: 16, color: c.textPrimary, fontWeight: '600' },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    protocolSection: { gap: 8, marginTop: 8 },
    protocolList: { gap: 8 },
    protocolRow: {
      gap: 2,
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    protocolRowHead: { fontSize: 14, fontWeight: '600', color: c.textPrimary },
    protocolRowSummary: { fontSize: 13, color: c.textSecondary },
    protocolRowError: { color: c.danger },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    primaryButton: {
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryDisabled: { backgroundColor: c.accentDisabled },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    conflictsButton: {
      paddingVertical: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
      alignItems: 'center',
    },
    conflictsButtonText: { fontSize: 16, fontWeight: '700', color: c.danger },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    warning: {
      fontSize: 15,
      fontWeight: '600',
      color: c.warning,
      backgroundColor: c.warningBg,
      padding: 12,
      borderRadius: 10,
    },
    trustPanel: {
      gap: 8,
      padding: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    trustTitle: { fontSize: 17, fontWeight: '700', color: c.textPrimary },
    trustBody: { fontSize: 14, color: c.textLabel },
    // Monospace so the fingerprint reads character-by-character (and a SR user can
    // compare it exactly against the out-of-band value).
    trustField: { fontSize: 14, color: c.textPrimary, fontFamily: 'monospace' },
    switchRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    switchLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
  });
