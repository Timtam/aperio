import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Linking,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  View,
} from 'react-native';

import { ALLOWED_LINK_SCHEMES, schemeOf } from '@aperio/shared';

import {
  clearContactsCache,
  getContactsSyncStatus,
  setContactsIncludeReadOnlyOnSync,
  setContactsSyncInterval,
  syncContactsNow,
  type ContactsSyncStatus,
} from '../api/contacts';
import { RadioGroup } from '../components/RadioGroup';
import { useTheme, useThemedStyles, type ThemeColors } from '../theme';

// Contacts settings (§10.5 / §10.6) — the mobile twin of the desktop
// ContactsPanel sync section: manual sync, the periodic interval, the
// include-read-only-directories toggle, the last-synced footer, a clear-cache
// action, and the standing privacy notice (the same text the one-shot connect
// modal shows). Screen-reader-first: the interval is a radio group, the
// directories toggle a single switch node, every action announces its result,
// and the provider policies are real accessible links opened via Linking.

/** The contacts sync-interval presets (minutes) — coarser than the main sync's
 *  cadence; default 60. Mirrors the desktop dropdown (up to 240). */
const INTERVAL_PRESETS: readonly number[] = [15, 30, 60, 120, 240];

/** Provider privacy policies surfaced in the standing notice. */
const PROVIDER_POLICIES: readonly { name: string; url: string }[] = [
  { name: 'Google', url: 'https://policies.google.com/privacy' },
  { name: 'Microsoft', url: 'https://privacy.microsoft.com/privacystatement' },
];

/** One accessible switch row: the Pressable owns role/checked/label/tap; the
 *  inner Switch is the visual indicator only. Matches TaskSettingsScreen. */
function SwitchRow({
  label,
  value,
  onToggle,
  disabled,
}: {
  label: string;
  value: boolean;
  onToggle: () => void;
  disabled?: boolean;
}) {
  const styles = useThemedStyles(makeStyles);
  const { colors } = useTheme();
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: value, disabled }}
      accessibilityLabel={label}
      disabled={disabled}
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

export default function ContactsSettingsScreen() {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [status, setStatus] = useState<ContactsSyncStatus | null>(null);
  const [busySync, setBusySync] = useState(false);
  const [busyClear, setBusyClear] = useState(false);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // Reflect the stored status whenever focused (a pref may have changed on
  // another device, or a pass landed while the screen was backgrounded).
  useFocusEffect(
    useCallback(() => {
      void getContactsSyncStatus()
        .then(setStatus)
        .catch(() => {});
    }, []),
  );

  const reload = useCallback(async () => {
    try {
      setStatus(await getContactsSyncStatus());
    } catch {
      // Best-effort; the footer keeps its last value.
    }
  }, []);

  const onSyncNow = useCallback(async () => {
    // Guard on the backend in-flight state too (a foreground pass may already be
    // running), not just the local busy flag — mirrors the desktop button.
    if (busySync || status?.in_flight) return;
    setBusySync(true);
    // Pass the effective UI value as an explicit override (the desktop pattern)
    // rather than null-read-the-pref: a flip-then-immediately-sync then can't
    // race the in-flight pref persist, and the announce below agrees with the
    // pass. The pass can take a few seconds (more with directories), so announce
    // that it started — the desktop's optimistic cue; the reload reflects done.
    const include = status?.include_read_only_on_sync ?? false;
    announce(
      include
        ? t('dialogs.settings.contacts.syncStartedFull')
        : t('dialogs.settings.contacts.syncStarted'),
    );
    try {
      await syncContactsNow(include);
      await reload();
    } catch (err) {
      announce(t('mobile.error', { message: errorMessage(err) }));
    } finally {
      setBusySync(false);
    }
  }, [announce, busySync, reload, status?.in_flight, status?.include_read_only_on_sync, t]);

  const onIntervalChange = useCallback(
    async (minutes: number) => {
      try {
        const clamped = await setContactsSyncInterval(minutes);
        await reload();
        announce(t('dialogs.settings.contacts.intervalChanged', { minutes: clamped }));
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, reload, t],
  );

  const onToggleIncludeReadOnly = useCallback(async () => {
    const next = !(status?.include_read_only_on_sync ?? false);
    // Optimistic flip so the switch responds immediately.
    setStatus((prev) => (prev ? { ...prev, include_read_only_on_sync: next } : prev));
    try {
      await setContactsIncludeReadOnlyOnSync(next);
      announce(
        next
          ? t('dialogs.settings.contacts.includeDirectoriesEnabled')
          : t('dialogs.settings.contacts.includeDirectoriesDisabled'),
      );
    } catch (err) {
      // Revert on failure.
      setStatus((prev) => (prev ? { ...prev, include_read_only_on_sync: !next } : prev));
      announce(t('mobile.error', { message: errorMessage(err) }));
    }
  }, [announce, status?.include_read_only_on_sync, t]);

  const onClearCache = useCallback(async () => {
    if (busyClear) return;
    setBusyClear(true);
    try {
      const count = await clearContactsCache();
      await reload();
      announce(t('dialogs.settings.contacts.cacheCleared', { count }));
    } catch {
      announce(t('dialogs.settings.contacts.cacheClearFailed'));
    } finally {
      setBusyClear(false);
    }
  }, [announce, busyClear, reload, t]);

  const openPolicy = useCallback(
    async (url: string) => {
      const scheme = schemeOf(url);
      if (scheme == null || !ALLOWED_LINK_SCHEMES.has(scheme)) return;
      try {
        await Linking.openURL(url);
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
        label: t('dialogs.settings.contacts.intervalOption', { count: min, minutes: min }),
      })),
    [t],
  );

  const lastSynced = useMemo(() => {
    if (!status?.last_synced_at) return t('dialogs.settings.contacts.neverSynced');
    const d = new Date(status.last_synced_at);
    if (Number.isNaN(d.getTime())) return t('dialogs.settings.contacts.neverSynced');
    const fmt = new Intl.DateTimeFormat(i18n.language, {
      dateStyle: 'medium',
      timeStyle: 'short',
    });
    return t('dialogs.settings.contacts.lastSynced', { time: fmt.format(d) });
  }, [status?.last_synced_at, i18n.language, t]);

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      {/* Sync */}
      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.contacts.syncTitle')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.contacts.syncBody')}
        </Text>

        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busySync || status?.in_flight === true }}
          accessibilityLabel={t('dialogs.settings.contacts.syncNow')}
          disabled={busySync || status?.in_flight === true}
          onPress={() => void onSyncNow()}
          style={({ pressed }) => [
            styles.primaryButton,
            pressed && styles.pressed,
            (busySync || status?.in_flight === true) && styles.disabled,
          ]}
        >
          <Text style={styles.primaryButtonText} importantForAccessibility="no">
            {busySync
              ? t('dialogs.settings.contacts.syncing')
              : t('dialogs.settings.contacts.syncNow')}
          </Text>
        </Pressable>

        <Text style={styles.footer} accessibilityRole="text">
          {lastSynced}
        </Text>

        <RadioGroup<number>
          label={t('dialogs.settings.contacts.intervalLabel')}
          value={status?.interval_minutes ?? 60}
          options={intervalOptions}
          onChange={(min) => void onIntervalChange(min)}
        />

        <SwitchRow
          label={t('dialogs.settings.contacts.includeDirectoriesLabel')}
          value={status?.include_read_only_on_sync ?? false}
          onToggle={() => void onToggleIncludeReadOnly()}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.contacts.includeDirectoriesHint')}
        </Text>
      </View>

      {/* Clear cache */}
      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.contacts.cacheTitle')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.contacts.cacheBody')}
        </Text>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busyClear }}
          accessibilityLabel={t('dialogs.settings.contacts.clearCache')}
          disabled={busyClear}
          onPress={() => void onClearCache()}
          style={({ pressed }) => [
            styles.dangerButton,
            pressed && styles.pressed,
            busyClear && styles.disabled,
          ]}
        >
          <Text style={styles.dangerButtonText} importantForAccessibility="no">
            {busyClear
              ? t('dialogs.settings.contacts.clearing')
              : t('dialogs.settings.contacts.clearCache')}
          </Text>
        </Pressable>
      </View>

      {/* Privacy (standing reference) */}
      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.contacts.privacyTitle')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.contacts.privacyBody')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.contacts.privacyProvidersIntro')}
        </Text>
        {PROVIDER_POLICIES.map((p) => (
          <Pressable
            key={p.name}
            accessibilityRole="link"
            accessibilityLabel={t('dialogs.settings.contacts.providerPolicyAria', {
              provider: p.name,
            })}
            onPress={() => void openPolicy(p.url)}
            style={({ pressed }) => [styles.link, pressed && styles.pressed]}
          >
            <Text style={styles.linkText} importantForAccessibility="no">
              {p.name === 'Google'
                ? t('dialogs.settings.contacts.providerPolicyGoogle')
                : t('dialogs.settings.contacts.providerPolicyMicrosoft')}
            </Text>
          </Pressable>
        ))}
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.contacts.providerPolicyOthers')}
        </Text>
      </View>
    </ScrollView>
  );
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 24 },
    section: { gap: 10 },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
    footer: { fontSize: 14, color: c.textSecondary, fontStyle: 'italic' },
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
    link: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    linkText: { fontSize: 15, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
    disabled: { opacity: 0.5 },
  });
