import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, Pressable, StyleSheet, Text } from 'react-native';

import { refreshExternalCache, syncNow } from '../api/sync';
import { useSyncStatusInfo } from '../state/syncStatusContext';
import { useThemedStyles, type ThemeColors } from '../theme';

// Header sync indicator — the desktop's status pill, surfaced on each main
// screen because the native tab bar can't carry an accessible label. Shows the
// current sync state (spoken via the label) and, on press, kicks a manual
// update: a peer sync round (if a target is configured) plus an external-cache
// warm. The start/end cue + the state update arrive via the existing observers,
// so this fires-and-forgets.

export function SyncStatusButton() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const info = useSyncStatusInfo();
  if (info == null) return null;

  const attention =
    info.tone === 'error' || info.tone === 'conflict' || info.tone === 'schema_too_old';

  const onPress = () => {
    AccessibilityInfo.announceForAccessibility(t('cacheRefresh.refreshing'));
    void syncNow('manual').catch(() => undefined);
    void refreshExternalCache().catch(() => undefined);
  };

  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${t('syncStatus.label')}: ${info.label}`}
      accessibilityHint={t('mobile.syncRefreshHint')}
      onPress={onPress}
      hitSlop={8}
      style={({ pressed }) => [styles.button, pressed && styles.pressed]}
    >
      <Text
        style={[styles.glyph, attention ? styles.attention : styles.normal]}
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
    attention: { color: c.danger },
  });
