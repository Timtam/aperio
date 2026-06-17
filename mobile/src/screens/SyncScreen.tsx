import { useCallback, useEffect, useState } from 'react';
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
  SyncStatus,
} from '../api/sync';

// Cross-device sync — a full desktop peer (same engine, statically-embedded
// adapters). This slice exposes the local-filesystem target (a shared folder
// both devices reach); webdav/sftp + OAuth kinds follow. Screen-reader-first:
// status is a live region, every control is labelled, results are announced.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SyncScreen() {
  const { t } = useTranslation();

  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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

  const configure = useCallback(async () => {
    const trimmed = path.trim();
    if (trimmed.length === 0) return;
    setError(null);
    setBusy(true);
    try {
      await configureSyncAdapter({ kind: 'local', path: trimmed });
      await refresh();
      announce(t('mobile.syncConfigured'));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
    }
  }, [announce, path, refresh, t]);

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
