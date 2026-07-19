import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text } from 'react-native';

import { refreshExternalCache, syncNow } from '../api/sync';
import {
  getCacheRefreshProgress,
  subscribeCacheRefreshProgress,
} from '../state/cacheRefreshProgress';
import { isSyncing, subscribeSyncActivity } from '../state/syncActivity';
import { useSyncStatusInfo } from '../state/syncStatusContext';
import {
  getRefreshErrorsSnapshot,
  subscribeRefreshErrors,
} from '../state/useRefreshErrors';
import { useThemedStyles, type ThemeColors } from '../theme';

// Header sync indicator — the desktop's status pill, surfaced on each main
// screen because the native tab bar has no slot for an extra custom accessible
// control (its tabs announce their own titles, but nothing else fits). Shows the
// current sync state (spoken via the label) and, on press, kicks a manual
// update: a peer sync round (if a target is configured) plus an external-cache
// warm. While a round runs it flips to "uploading" via the syncActivity
// subscription — the root status poll (30s) is too coarse to catch the
// seconds-long in-flight window. Subscribing HERE (not at the root) keeps the
// re-render to this leaf, so the nav shell — and the VoiceOver cursor — stays put.

export function SyncStatusButton() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const info = useSyncStatusInfo();
  const [syncing, setSyncing] = useState(isSyncing());
  useEffect(() => subscribeSyncActivity(setSyncing), []);
  const [progress, setProgress] = useState(getCacheRefreshProgress());
  useEffect(() => subscribeCacheRefreshProgress(setProgress), []);
  // Per-account refresh errors: this header button sits on every landing
  // screen, so it is the persistent visible+focusable cue that some account
  // is silently serving stale data (the detail rows live on the Sync screen).
  const [refreshErrs, setRefreshErrs] = useState(getRefreshErrorsSnapshot());
  useEffect(() => subscribeRefreshErrors(setRefreshErrs), []);
  if (info == null) return null;

  const attention =
    info.tone === 'error' || info.tone === 'conflict' || info.tone === 'schema_too_old';
  // A conflict/error/schema badge outranks a transient upload (matching the
  // desktop tone priority), so only override the otherwise-benign synced/off state.
  const refreshErrorAttention = refreshErrs.length > 0 && !attention;
  const externalRefreshing = progress.refreshing && !attention;
  const showSyncing = syncing && !attention;
  // While a warm pass runs, the external-refresh progress (fetched X of N) takes
  // the label, so focusing the indicator speaks the LIVE progress — no chatter.
  const externalLabel =
    externalRefreshing && progress.total != null && progress.fetched != null
      ? t('cacheRefresh.progress', {
          fetched: progress.fetched,
          total: progress.total,
        })
      : externalRefreshing
        ? t('cacheRefresh.refreshing')
        : null;
  const baseLabel = attention
    ? info.label
    : (externalLabel ?? (showSyncing ? t('syncStatus.uploading') : info.label));
  // Refresh errors ride along as a label suffix (they describe a persistent
  // state, so they must not displace the live progress/engine label).
  const label = refreshErrorAttention
    ? `${baseLabel}. ${t(
        refreshErrs.some((r) => r.auth_suspected)
          ? 'refreshErrors.bannerAuth'
          : 'refreshErrors.banner',
      )}`
    : baseLabel;

  const onPress = () => {
    AccessibilityInfo.announceForAccessibility(t('cacheRefresh.refreshing'));
    void syncNow('manual').catch(() => undefined);
    void refreshExternalCache().catch(() => undefined);
  };

  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${t('syncStatus.label')}: ${label}`}
      accessibilityHint={t('mobile.syncRefreshHint')}
      onPress={onPress}
      hitSlop={8}
      style={({ pressed }) => [styles.button, pressed && styles.pressed]}
    >
      <Text
        style={[
          styles.glyph,
          attention || refreshErrorAttention
            ? styles.attention
            : showSyncing || externalRefreshing
              ? styles.syncing
              : styles.normal,
        ]}
        importantForAccessibility="no"
      >
        {info.badge != null ? String(info.badge) : '↻'}
      </Text>
    </Pressable>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    button: { paddingHorizontal: 10, paddingVertical: 4 },
    pressed: { opacity: 0.6 },
    glyph: { fontSize: 18, fontWeight: '700' },
    normal: { color: c.link },
    // Distinct from the idle link colour so sighted users get a cue that a round
    // is in progress (the label carries it for screen-reader users).
    syncing: { color: c.accent },
    attention: { color: c.danger },
  });
