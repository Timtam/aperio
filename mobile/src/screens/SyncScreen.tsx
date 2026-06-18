import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  configureSyncAdapter,
  syncNow,
  syncStatus,
  SyncAdapterConfig,
  SyncStatus,
} from '../api/sync';
import { RadioGroup } from '../components/RadioGroup';

// Cross-device sync — a full desktop peer (same engine, statically-embedded
// adapters). This screen exposes the password-only targets: a local shared
// folder, WebDAV (Nextcloud/ownCloud), and FTPS. SFTP (host-key trust flow) +
// the OAuth kinds follow. Screen-reader-first: the kind is a radio group, every
// field is its own labelled stop, status is a live region, results announced.

type SyncKind = 'local' | 'webdav' | 'ftp';
type FtpMode = 'explicit' | 'implicit' | 'plain';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SyncScreen() {
  const { t } = useTranslation();

  const [status, setStatus] = useState<SyncStatus | null>(null);
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

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const refresh = useCallback(async () => {
    try {
      setStatus(await syncStatus());
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const kindOptions = useMemo(
    () => [
      { value: 'local' as const, label: t('dialogs.settings.sync.adapterKindLocal') },
      { value: 'webdav' as const, label: t('dialogs.settings.sync.adapterKindWebdav') },
      { value: 'ftp' as const, label: t('dialogs.settings.sync.adapterKindFtp') },
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
    // ftp
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

  const lastSynced =
    status?.last_synced_at != null
      ? new Date(status.last_synced_at).toLocaleString()
      : t('mobile.syncNever');

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
});
