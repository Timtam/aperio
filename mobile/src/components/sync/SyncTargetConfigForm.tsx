import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  acceptRemoteDataset,
  adoptLocalDataset,
  configureSyncAdapter,
  forgetSftpHostKey,
  previewSftpHostKey,
  previewSyncTarget,
  refreshExternalCache,
  trustSftpHostKey,
  HostKeyPreview,
  SyncAdapterConfig,
  SyncDeviceSummary,
  SyncPreview,
} from '../../api/sync';
import { connectSyncOAuth } from '../../api/oauth';
import { formatLongDateTime } from '../../intl/dateFormat';
import { useThemedStyles, type ThemeColors } from '../../theme';
import { AppDialog } from '../AppDialog';
import { RadioGroup } from '../RadioGroup';

type SyncKind = 'local' | 'webdav' | 'ftp' | 'dropbox' | 'googledrive' | 'sftp';
type FtpMode = 'explicit' | 'implicit' | 'plain';
type SftpAuth = 'password' | 'key';

/** What a successful connect produced — handed to the host so it can decide
 *  what to do next (refresh its status, advance a wizard, …). */
export interface SyncConnectOutcome {
  /** `true` when we JOINED an existing remote dataset (restore); `false` when
   *  we initialised a fresh one / configured a target (create). */
  joined: boolean;
}

export interface SyncTargetConfigFormProps {
  /** Called after a connect succeeds. The form has cleared its own drafts and
   *  warmed the external cache; the host owns whatever comes next (e.g. a
   *  status refresh, or advancing the first-launch wizard). */
  onConnected: (outcome: SyncConnectOutcome) => void;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * The sync-target CONFIGURATION form (DESIGN.md §19.11) — adapter-kind radio,
 * per-kind fields (local / WebDAV / FTPS / Dropbox / Google Drive / SFTP),
 * OAuth sign-in, the §19.5 SFTP host-key TOFU trust panel, the device name,
 * and the unified "Connect" that previews the target then joins it (restore)
 * or initialises it (create), with optional E2E.
 *
 * Extracted from `SyncScreen` so the Settings → Sync screen AND the
 * first-launch wizard share ONE implementation. The component owns all of its
 * own state + a11y focus management; the host supplies `status` + `onConnected`.
 * Screen-reader-first: the kind is a radio group, every field is its own
 * labelled stop, the trust panel is an assertive live region, results announced.
 */
export function SyncTargetConfigForm({ onConnected }: SyncTargetConfigFormProps) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // §19.11 preview + onboarding drafts.
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const previewRef = useRef<SyncPreview | null>(null);
  previewRef.current = preview;
  const [joinPassphrase, setJoinPassphrase] = useState(''); // join-existing E2E passphrase
  const [joinDialogOpen, setJoinDialogOpen] = useState(false);
  const [joinDeviceName, setJoinDeviceName] = useState(''); // optional name for meta.json
  const [createE2e, setCreateE2e] = useState(false);
  // §19.5 SFTP host-key trust panel.
  const [pendingTrust, setPendingTrust] = useState<HostKeyPreview | null>(null);

  // a11y focus anchors.
  const trustRef = useRef<Text>(null);
  const sftpConnectRef = useRef<View>(null);
  const joinPanelRef = useRef<Text>(null);
  const emptyPanelRef = useRef<Text>(null);

  const focusSftpConnect = useCallback(() => {
    const tag = sftpConnectRef.current
      ? findNodeHandle(sftpConnectRef.current)
      : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const isOAuthKind = kind === 'dropbox' || kind === 'googledrive';
  const isSftp = kind === 'sftp';
  const sftpPortParsed = Number(sftpPort.trim());
  const sftpPortNum =
    sftpPort.trim().length > 0 &&
    Number.isInteger(sftpPortParsed) &&
    sftpPortParsed > 0 &&
    sftpPortParsed <= 65535
      ? sftpPortParsed
      : 22;

  const kindOptions = useMemo(
    () => [
      { value: 'webdav' as const, label: t('dialogs.settings.sync.adapterKindWebdav') },
      { value: 'ftp' as const, label: t('dialogs.settings.sync.adapterKindFtp') },
      { value: 'dropbox' as const, label: t('dialogs.settings.sync.adapterKindDropbox') },
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
      { value: 'password' as const, label: t('dialogs.settings.sync.adapterSftpAuthPassword') },
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

  const oauthSignInLabel =
    kind === 'dropbox'
      ? t('dialogs.settings.sync.adapterDropboxSignIn')
      : t('dialogs.settings.sync.adapterGoogledriveSignIn');
  const oauthSigningInLabel =
    kind === 'dropbox'
      ? t('dialogs.settings.sync.adapterDropboxSigningIn')
      : t('dialogs.settings.sync.adapterGoogledriveSigningIn');

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
        if (p.compatibility.kind === 'app_too_old') {
          setError(t('dialogs.settings.sync.errorSchemaTooOld'));
          announce(t('dialogs.settings.sync.errorSchemaTooOld'));
          return;
        }
        if (p.e2e_enabled) {
          setJoinPassphrase('');
          setJoinDialogOpen(true);
          announce(t('dialogs.settings.sync.e2eRemoteRequiresPassphrase'));
          return;
        }
        const report = await acceptRemoteDataset(config, device, null);
        setPreview(null);
        setJoinPassphrase('');
        setCreateE2e(false);
        void refreshExternalCache().catch(() => undefined);
        onConnected({ joined: true });
        announce(
          t('dialogs.settings.sync.onboardingDone', { count: report.device_count }),
        );
        return;
      }
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
      onConnected({ joined: false });
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
    onConnected,
    preview,
    t,
  ]);

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
      void refreshExternalCache().catch(() => undefined);
      onConnected({ joined: true });
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
  }, [announce, buildConfig, joinDeviceName, joinPassphrase, onConnected, t]);

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
                onConnected({ joined: false });
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
  }, [announce, buildConfig, joinDeviceName, onConnected, t]);

  // Drop a stale preview whenever any field feeding the config changes.
  useEffect(() => {
    if (previewRef.current != null) {
      announce(t('mobile.syncPreviewStale'));
    }
    setPreview(null);
    setJoinPassphrase('');
    setCreateE2e(false);
  }, [kind, path, url, host, port, ftpPath, mode, user, password, announce, t]);

  // Move SR focus onto the join/empty panel when it appears.
  useEffect(() => {
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
      onConnected({ joined: false });
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
    onConnected,
    t,
  ]);

  const buildSftpConfig = useCallback((): Extract<
    SyncAdapterConfig,
    { kind: 'sftp' }
  > | null => {
    const h = sftpHost.trim();
    const u = sftpUser.trim();
    const p = sftpPath.trim();
    if (h.length === 0 || u.length === 0 || p.length === 0) return null;
    const base = {
      kind: 'sftp' as const,
      host: h,
      port: sftpPortNum,
      user: u,
      path: p,
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

  const activateSftp = useCallback(async () => {
    const config = buildSftpConfig();
    if (config == null) return;
    await configureSyncAdapter(config);
    onConnected({ joined: false });
    announce(t('mobile.syncConfigured'));
  }, [announce, buildSftpConfig, onConnected, t]);

  const connectSftp = useCallback(async () => {
    const config = buildSftpConfig();
    if (config == null) {
      setError(t('mobile.sftpFieldsRequired'));
      announce(t('mobile.sftpFieldsRequired'));
      return;
    }
    setError(null);
    setPendingTrust(null);
    setBusy(true);
    try {
      const trust = await previewSftpHostKey(config.host, config.port ?? 22);
      if (trust.status.kind === 'unchanged') {
        await activateSftp();
      } else {
        setPendingTrust(trust);
        const title =
          trust.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustChangedTitle')
            : t('dialogs.settings.sync.sftpTrustNewTitle');
        announce(
          `${title} ${t('dialogs.settings.sync.sftpTrustPresentedLabel')}: ${trust.fingerprint}`,
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

  const confirmTrust = useCallback(async () => {
    if (pendingTrust == null) return;
    setError(null);
    setBusy(true);
    try {
      await trustSftpHostKey(pendingTrust.host_port, pendingTrust.fingerprint);
      setPendingTrust(null);
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

  const forgetPin = useCallback(async () => {
    const h = sftpHost.trim();
    if (h.length === 0) return;
    try {
      await forgetSftpHostKey(`${h}:${sftpPortNum}`);
      announce(t('dialogs.settings.sync.sftpForgetPinDone'));
    } catch (err) {
      announce(t('mobile.error', { message: errorMessage(err) }));
    }
  }, [announce, sftpHost, sftpPortNum, t]);

  // Move SR focus onto the trust panel when it appears.
  useEffect(() => {
    if (pendingTrust == null) return;
    const tag = trustRef.current ? findNodeHandle(trustRef.current) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [pendingTrust]);

  return (
    <>
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
              <Text style={styles.label}>{t('dialogs.settings.sync.adapterDropboxPath')}</Text>
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
                accessibilityLabel={t('dialogs.settings.sync.adapterGoogledriveFolderName')}
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
              <Text style={styles.label}>{t('dialogs.settings.sync.adapterSftpPassword')}</Text>
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
                <Text style={styles.label}>{t('dialogs.settings.sync.adapterSftpKeyPath')}</Text>
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
              {t('dialogs.settings.sync.sftpTrustStoredLabel')}: {pendingTrust.status.stored}
            </Text>
          )}
          <Text style={styles.trustField}>
            {t('dialogs.settings.sync.sftpTrustPresentedLabel')}: {pendingTrust.fingerprint}
          </Text>
          <Text style={styles.hint}>{t('dialogs.settings.sync.sftpTrustVerifyHint')}</Text>
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
                  <Text style={styles.label}>{t('dialogs.settings.sync.e2ePassphrase')}</Text>
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
                      time: formatLongDateTime(
                        new Date(preview.snapshot_timestamp),
                        i18n.language,
                      ),
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

              <Pressable
                accessibilityRole="button"
                accessibilityState={{ disabled: busy }}
                accessibilityLabel={t('dialogs.settings.sync.previewAdoptButton')}
                disabled={busy}
                onPress={confirmOverwrite}
                style={({ pressed }) => [styles.conflictsButton, pressed && styles.pressed]}
              >
                <Text style={styles.conflictsButtonText}>
                  {t('dialogs.settings.sync.previewAdoptButton')}
                </Text>
              </Pressable>
            </View>
          )}
        </>
      )}

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="polite">
          {error}
        </Text>
      )}

      {/* Encrypted-dataset join: a focus-trapping popup that owns the passphrase
          entry. */}
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
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
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
