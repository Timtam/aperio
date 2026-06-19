import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  Pressable,
  ScrollView,
  Share,
  StyleSheet,
  Switch,
  Text,
  View,
} from 'react-native';

import {
  clearLogs,
  collectLogs,
  getLogLevel,
  getRecentLogs,
  logsDirPath,
  setLogLevel,
  type LogLevel,
} from '../api/logs';
import { RadioGroup } from '../components/RadioGroup';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Diagnostics / logs (§ Diagnostics) — the mobile twin of the desktop LogsPanel.
// Choose the detail level (device-local), view the latest lines, and export the
// (redacted by default) log via the OS share sheet so a blind user can send it
// for support. Screen-reader-first: the level is a radio group, the redact
// toggle a single switch node, every action announces its result, the viewer is
// a plain selectable text (not a live region).

const LEVELS: readonly LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace'];

function isLogLevel(v: string): v is LogLevel {
  return (LEVELS as readonly string[]).includes(v);
}

/** One accessible switch row (Pressable owns role/checked/label; the Switch is
 *  the visual indicator only). Matches TaskSettingsScreen / ContactsSettings. */
function SwitchRow({
  label,
  value,
  onToggle,
}: {
  label: string;
  value: boolean;
  onToggle: () => void;
}) {
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: value }}
      accessibilityLabel={label}
      onPress={onToggle}
      style={({ pressed }) => [styles.switchRow, pressed && styles.pressed]}
    >
      <Text style={styles.switchLabel} importantForAccessibility="no">
        {label}
      </Text>
      <View pointerEvents="none">
        <Switch
          value={value}
          trackColor={{ false: colors.border, true: colors.accent }}
          importantForAccessibility="no"
          accessibilityElementsHidden
        />
      </View>
    </Pressable>
  );
}

export default function LogsScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [level, setLevel] = useState<LogLevel>('info');
  const [redact, setRedact] = useState(true);
  const [recent, setRecent] = useState('');
  const [dirPath, setDirPath] = useState('');
  const [busyExport, setBusyExport] = useState(false);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const loadRecent = useCallback(async () => {
    try {
      setRecent(await getRecentLogs());
    } catch {
      // Best-effort — leave the prior content.
    }
  }, []);

  useFocusEffect(
    useCallback(() => {
      void getLogLevel()
        .then((lv) => {
          if (isLogLevel(lv)) setLevel(lv);
        })
        .catch(() => {});
      void logsDirPath()
        .then(setDirPath)
        .catch(() => {});
      void loadRecent();
    }, [loadRecent]),
  );

  const onLevelChange = useCallback((next: LogLevel) => {
    setLevel(next);
    void setLogLevel(next).catch(() => {
      // Revert handled by the next focus reload; the optimistic set already
      // reflects intent for the session.
    });
  }, []);

  const onShare = useCallback(async () => {
    if (busyExport) return;
    setBusyExport(true);
    try {
      const text = await collectLogs(redact);
      if (text.trim().length === 0) {
        announce(t('dialogs.settings.logs.empty'));
        return;
      }
      const result = await Share.share({ message: text });
      if (result.action === Share.sharedAction) {
        announce(t('dialogs.settings.logs.exported'));
      }
    } catch (err) {
      announce(t('mobile.error', { message: err instanceof Error ? err.message : String(err) }));
    } finally {
      setBusyExport(false);
    }
  }, [announce, busyExport, redact, t]);

  const onClear = useCallback(() => {
    Alert.alert(t('dialogs.settings.logs.clear'), t('dialogs.settings.logs.clearConfirm'), [
      { text: t('mobile.cancel'), style: 'cancel' },
      {
        text: t('dialogs.settings.logs.clear'),
        style: 'destructive',
        onPress: () => {
          void (async () => {
            try {
              await clearLogs();
              await loadRecent();
              announce(t('dialogs.settings.logs.cleared'));
            } catch (err) {
              announce(
                t('mobile.error', { message: err instanceof Error ? err.message : String(err) }),
              );
            }
          })();
        },
      },
    ]);
  }, [announce, loadRecent, t]);

  const levelOptions = useMemo(
    () => LEVELS.map((lv) => ({ value: lv, label: t(`dialogs.settings.logs.level.${lv}`) })),
    [t],
  );

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <Text style={styles.hint} accessibilityRole="text">
        {t('dialogs.settings.logs.hint')}
      </Text>

      {/* Detail level */}
      <View style={styles.section}>
        <RadioGroup<LogLevel>
          label={t('dialogs.settings.logs.levelLabel')}
          value={level}
          options={levelOptions}
          onChange={onLevelChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.logs.levelHint')}
        </Text>
      </View>

      {/* Export & manage */}
      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.logs.exportHeading')}
        </Text>
        <SwitchRow
          label={t('dialogs.settings.logs.redact')}
          value={redact}
          onToggle={() => setRedact((r) => !r)}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.logs.redactHint')}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busyExport }}
          accessibilityLabel={t('dialogs.settings.logs.export')}
          disabled={busyExport}
          onPress={() => void onShare()}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.pressed,
            busyExport && styles.disabled,
          ]}
        >
          <Text style={styles.primaryButtonText} importantForAccessibility="no">
            {t('dialogs.settings.logs.export')}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.logs.clear')}
          onPress={onClear}
          style={({ pressed }) => [styles.dangerButton, pressed && styles.pressed]}
        >
          <Text style={styles.dangerButtonText} importantForAccessibility="no">
            {t('dialogs.settings.logs.clear')}
          </Text>
        </Pressable>
        {dirPath.length > 0 && (
          <Text style={styles.location} accessibilityRole="text">
            {t('dialogs.settings.logs.location', { path: dirPath })}
          </Text>
        )}
      </View>

      {/* Recent log viewer */}
      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.logs.viewHeading')}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.logs.refresh')}
          onPress={() => void loadRecent()}
          style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
        >
          <Text style={styles.ghostButtonText} importantForAccessibility="no">
            {t('dialogs.settings.logs.refresh')}
          </Text>
        </Pressable>
        <Text style={styles.viewer} accessibilityRole="text" selectable>
          {recent.length > 0 ? recent : t('dialogs.settings.logs.empty')}
        </Text>
      </View>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 24 },
    section: { gap: 10 },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
    location: { fontSize: 13, color: c.textSecondary },
    primaryButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    dangerButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
      alignItems: 'center',
    },
    dangerButtonText: { fontSize: 16, fontWeight: '700', color: c.danger },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
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
    viewer: {
      fontSize: 12,
      color: c.textPrimary,
      fontFamily: 'monospace',
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceSubtle,
    },
    pressed: { opacity: 0.7 },
    disabled: { opacity: 0.5 },
  });
