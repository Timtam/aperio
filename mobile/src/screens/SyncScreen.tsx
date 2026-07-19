import { useFocusEffect, useNavigation } from '@react-navigation/native';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  adoptRemoteEncryption,
  cacheRefreshStatus,
  changeSyncPassphrase,
  compactNow,
  disableSyncEncryption,
  enableSyncEncryption,
  refreshExternalCache,
  resumeStaleDevice,
  clearSyncLog,
  listSyncLog,
  syncConflictCount,
  syncNow,
  syncStatus,
  disconnectSync,
  getSyncAdapterSummary,
  CacheRefreshStatus,
  SyncAdapterSummary,
  SyncLogEntry,
  SyncStatus,
} from '../api/sync';
import { listAccounts } from '../api/accounts';
import { setUserPref } from '../api/prefs';
import { FormScrollView } from '../components/FormScrollView';
import { RadioGroup } from '../components/RadioGroup';
import { SyncTargetConfigForm } from '../components/sync/SyncTargetConfigForm';
import { formatLongDateTime } from '../intl/dateFormat';
import { useRefreshErrors } from '../state/useRefreshErrors';
import { useThemedStyles, type ThemeColors } from '../theme';
import CalFfi from '../../modules/cal-ffi';

// Cross-device sync — a full desktop peer (same engine, statically-embedded
// adapters). The adapter-target CONFIGURATION form (kind picker, per-kind
// fields, OAuth, SFTP host-key trust, preview → join/init) lives in
// `SyncTargetConfigForm`, shared with the first-launch wizard. This screen
// keeps the status display, periodic-interval + compaction controls, E2E
// management, the stale-resume + adopt-encryption banners, and the protocol.

const PREF_SYNC_INTERVAL_MINUTES = 'sync.intervalMinutes';
const INTERVAL_PRESETS: readonly number[] = [1, 5, 15, 30, 60, 240];

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SyncScreen() {
  const { t, i18n } = useTranslation();
  const navigation = useNavigation();
  const styles = useThemedStyles(makeStyles);

  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [conflictCount, setConflictCount] = useState(0);
  const [syncLog, setSyncLog] = useState<SyncLogEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [busyCompact, setBusyCompact] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [adapterSummary, setAdapterSummary] = useState<SyncAdapterSummary | null>(null);
  const [cacheStatus, setCacheStatus] = useState<CacheRefreshStatus | null>(null);
  const [hasExternalAccounts, setHasExternalAccounts] = useState(false);
  // Account display names for the refresh-error rows (id → name).
  const [accountNameById, setAccountNameById] = useState<Map<string, string>>(
    new Map(),
  );
  // Per-account refresh-error surface (silent-staleness warning).
  const { errorsByAccount } = useRefreshErrors();
  // §19.7 E2E management drafts (only meaningful once a target is configured).
  const [e2ePassphrase, setE2ePassphrase] = useState(''); // enable-E2E passphrase
  const [changeOldPp, setChangeOldPp] = useState(''); // rotate: current passphrase
  const [changeNewPp, setChangeNewPp] = useState(''); // rotate: new passphrase
  const [disablePp, setDisablePp] = useState(''); // disable: current passphrase
  const [adoptPp, setAdoptPp] = useState(''); // adopt peer-enabled E2E passphrase
  const adoptBannerRef = useRef<Text>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

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
      setAdapterSummary(await getSyncAdapterSummary().catch(() => null));
      setConflictCount(await syncConflictCount().catch(() => 0));
      setSyncLog(await listSyncLog(100).catch(() => []));
      setCacheStatus(await cacheRefreshStatus().catch(() => null));
      const accounts = await listAccounts().catch(() => []);
      setHasExternalAccounts(accounts.some((a) => a.adapter_kind !== 'local'));
      setAccountNameById(new Map(accounts.map((a) => [a.id, a.display_name])));
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  // Refresh on every focus (not just mount) so a background-triggered round's
  // result shows when the user opens this screen.
  useFocusEffect(
    useCallback(() => {
      void refresh();
    }, [refresh]),
  );

  // Subscribe to the native external-cache warm-pass status WHILE FOCUSED.
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

  const refreshCache = useCallback(async () => {
    try {
      await refreshExternalCache();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    }
  }, [announce, t]);

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

  const kindLabel = useCallback(
    (kind: string): string =>
      t(
        `dialogs.settings.sync.adapterKind${kind.charAt(0).toUpperCase()}${kind.slice(1)}`,
        { defaultValue: kind },
      ),
    [t],
  );

  const onDisconnect = useCallback(() => {
    Alert.alert(
      t('dialogs.settings.sync.adapterDisconnect'),
      t('dialogs.settings.sync.adapterDisconnectConfirm'),
      [
        { text: t('mobile.cancel'), style: 'cancel' },
        {
          text: t('dialogs.settings.sync.adapterDisconnect'),
          style: 'destructive',
          onPress: () => {
            void (async () => {
              setBusy(true);
              try {
                await disconnectSync();
                await refresh();
                announce(t('dialogs.settings.sync.adapterDisconnected'));
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
  }, [announce, refresh, t]);

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

  // §19.7 — enable E2E (irreversible without the passphrase, gated by a confirm).
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

  // §19.7 — rotate the passphrase (data unchanged; future joins use the new one).
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

  // §19.7 — adopt encryption a peer turned on.
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
      await syncNow();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusy(false);
      await refresh();
    }
  }, [adoptPp, announce, refresh, t]);

  // §19.10 — re-onboard a device that fell behind the compaction window.
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

  // Move SR focus onto the adopt banner when it appears.
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
      ? formatLongDateTime(new Date(status.last_synced_at), i18n.language)
      : t('mobile.syncNever');

  const cacheStatusLine = cacheStatus?.refreshing
    ? t('cacheRefresh.refreshing')
    : cacheStatus?.last_refreshed_at != null
      ? t('cacheRefresh.lastUpdated', {
          time: formatLongDateTime(new Date(cacheStatus.last_refreshed_at), i18n.language),
        })
      : t('cacheRefresh.never');

  return (
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
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

      {/* Connected-target card + Disconnect. */}
      {status?.configured && adapterSummary != null && (
        <View style={styles.field}>
          <Text style={styles.label} accessibilityRole="text">
            {t('dialogs.settings.sync.connectedSummary', {
              kind: kindLabel(adapterSummary.kind),
              detail: adapterSummary.detail || '–',
            })}
          </Text>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.settings.sync.adapterDisconnect')}
            accessibilityState={{ disabled: busy }}
            disabled={busy}
            onPress={onDisconnect}
            style={({ pressed }) => [styles.ghostButton, pressed && { opacity: 0.6 }]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.settings.sync.adapterDisconnect')}
            </Text>
          </Pressable>
        </View>
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

      {/* §19.9 — the dataset was written by a newer Aperio; this build can't apply it. */}
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

      {/* §19.10 — this device went stale; offer a full re-onboard. */}
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

      {/* §19.7 — adopt encryption a peer turned on. */}
      {adoptRequired && (
        <View style={styles.field}>
          <Text ref={adoptBannerRef} style={styles.label} accessibilityRole="header">
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

      {/* Foreground periodic-sync interval — configured-only. */}
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

      {/* Manual compaction (§19.10) — configured-only. */}
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

      {/* Unresolved sync conflicts → the resolution screen. */}
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
            <View style={styles.field}>
              <Text style={styles.label} accessibilityRole="header">
                {t('dialogs.settings.sync.passphraseChangeTitle')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.passphraseChangeHint')}
              </Text>
              <Text style={styles.label}>{t('dialogs.settings.sync.passphraseChangeOld')}</Text>
              <TextInput
                style={styles.input}
                value={changeOldPp}
                onChangeText={setChangeOldPp}
                accessibilityLabel={t('dialogs.settings.sync.passphraseChangeOld')}
                secureTextEntry
                autoCapitalize="none"
                autoCorrect={false}
              />
              <Text style={styles.label}>{t('dialogs.settings.sync.passphraseChangeNew')}</Text>
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

            <View style={styles.field}>
              <Text style={styles.label} accessibilityRole="header">
                {t('dialogs.settings.sync.disableE2eAction')}
              </Text>
              <Text style={styles.hint} accessibilityRole="text">
                {t('dialogs.settings.sync.disableE2eHint')}
              </Text>
              <Text style={styles.label}>{t('dialogs.settings.sync.passphraseChangeOld')}</Text>
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
            <Text style={styles.label}>{t('dialogs.settings.sync.e2ePassphrase')}</Text>
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

      {/* Connection setup / onboarding — ONLY while no target is configured. The
          shared config form (also used by the first-launch wizard) owns the
          kind picker, per-kind fields, OAuth, SFTP trust, and preview→join/init. */}
      {!status?.configured && (
        <SyncTargetConfigForm onConnected={() => void refresh()} />
      )}

      {/* External data — manual refresh + live status for the external cache. */}
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
          {/* Refresh-error surface: which accounts are failing to update,
              per-container details + how stale the visible data is, and a
              re-enter-password hint for auth-shaped errors. Linear rows —
              a screen reader walks them naturally. Silent when healthy. */}
          {[...errorsByAccount.values()].map((acc) => (
            <View key={acc.account_id} style={styles.refreshErrorBox}>
              <Text style={styles.refreshErrorTitle} accessibilityRole="header">
                {t('refreshErrors.heading', {
                  name: accountNameById.get(acc.account_id) ?? acc.account_id,
                })}
              </Text>
              {acc.auth_suspected && (
                <Text style={styles.refreshErrorAuth} accessibilityRole="text">
                  {t('refreshErrors.authHint')}
                </Text>
              )}
              {acc.errors.map((err) => (
                <Text
                  key={`${err.scope}:${err.container_id}`}
                  style={styles.refreshErrorEntry}
                  accessibilityRole="text"
                >
                  {t('refreshErrors.entry', {
                    container:
                      err.container_name ??
                      t(`refreshErrors.scope.${err.scope}`, {
                        defaultValue: err.scope,
                      }),
                    error: err.error,
                  })}{' '}
                  {err.last_success_at
                    ? t('refreshErrors.lastSuccess', {
                        time: formatLongDateTime(
                          new Date(err.last_success_at),
                          i18n.language,
                        ),
                      })
                    : t('refreshErrors.neverSucceeded')}
                </Text>
              ))}
            </View>
          ))}
        </View>
      )}

      {/* Protocol — recent sync rounds (newest first). */}
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
              const when = formatLongDateTime(new Date(entry.recorded_at), i18n.language);
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
    </FormScrollView>
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
    refreshErrorBox: {
      marginTop: 10,
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.danger,
      backgroundColor: c.surfaceAlt,
      gap: 6,
    },
    refreshErrorTitle: { fontSize: 15, fontWeight: '700', color: c.danger },
    refreshErrorAuth: { fontSize: 14, fontWeight: '600', color: c.textPrimary },
    refreshErrorEntry: { fontSize: 14, color: c.textSecondary },
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
  });
