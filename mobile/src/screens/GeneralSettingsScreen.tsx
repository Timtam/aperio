import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { RadioGroup } from '../components/RadioGroup';
import { SoundSelect } from '../components/SoundSelect';
import { SwitchRow } from '../components/SwitchRow';
import {
  applyLanguageChoice,
  readLanguageChoice,
  writeLanguageChoice,
  type LanguageChoice,
} from '../settings/language';
import { readWeekStart, writeWeekStart, type WeekStart } from '../settings/weekStart';
import { useAppBadgePref } from '../state/appBadge';
import { useBackgroundSyncPref } from '../state/backgroundSync';
import { useHapticsPref } from '../state/haptics';
import { readTaskBehaviour, writeDayViewMode } from '../state/taskBehaviour';
import { useSoundPref } from '../state/useSoundPref';
import { useThemedStyles, type ThemeColors } from '../theme';
import { useThemeModePref, type ThemeModeChoice } from '../theme/themeMode';

// General settings — the mobile twin of the desktop Settings "General" tab,
// pushed from the Settings hub so the hub stays a clean list of destinations
// rather than mixing inline controls with links. Language override, week start,
// and the global default reminder sound; each section title is an accessibility
// heading (so screen-reader heading navigation reaches them). The desktop's
// system-tray section is Tauri-only and intentionally absent on mobile.

export default function GeneralSettingsScreen() {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [language, setLanguage] = useState<LanguageChoice>('system');
  const [weekStart, setWeekStart] = useState<WeekStart>(1);
  // Calendar day/week layout (synced `calendar.dayViewMode`) — a view preference
  // like week-start, so it lives here (the mobile twin of the desktop Calendars
  // panel's setting), not under Tasks. Also switchable from the calendar toolbar.
  const [dayViewMode, setDayViewMode] = useState<'grid' | 'list'>('grid');
  // The global default reminder sound (§14.4 root). System/Silent only on mobile
  // — Custom needs an asset store the host lacks; a custom value synced from
  // desktop still round-trips and is shown read-only by SoundSelect.
  const globalSound = useSoundPref('sound.global');
  // Device-local light/dark/system theme mode (default: follow the OS).
  const [themeMode, setThemeMode] = useThemeModePref();
  // Device-local haptic feedback for the external-sync start/end cues (default on).
  const [haptics, setHaptics] = useHapticsPref();
  // Device-local app-icon badge (today's open tasks + upcoming events; default on).
  const [appBadge, setAppBadge] = useAppBadgePref();
  // Device-local OS background sync (default on) — wakes the app to sync while
  // it's backgrounded/closed; the OS decides the exact timing.
  const [bgSync, setBgSync] = useBackgroundSyncPref();

  // Reflect the stored choices whenever the screen is focused (they may have
  // been applied on launch — or changed on another device — before this mounted).
  useFocusEffect(
    useCallback(() => {
      void readLanguageChoice().then(setLanguage);
      void readWeekStart().then(setWeekStart);
      void readTaskBehaviour().then((b) => setDayViewMode(b.dayViewMode));
    }, []),
  );

  const onLanguageChange = useCallback((next: LanguageChoice) => {
    setLanguage(next);
    void writeLanguageChoice(next);
    void applyLanguageChoice(next);
  }, []);

  const onWeekStartChange = useCallback((next: WeekStart) => {
    setWeekStart(next);
    void writeWeekStart(next);
  }, []);

  const onDayViewModeChange = useCallback((next: 'grid' | 'list') => {
    setDayViewMode(next);
    void writeDayViewMode(next);
  }, []);

  // Localized full weekday names for the picker. 7 Jan 2024 is a Sunday (index
  // 0 = date-fns/`view.weekStart` value 0), so option d maps to that weekday.
  const weekdayOptions = useMemo(() => {
    const fmt = new Intl.DateTimeFormat(i18n.language, { weekday: 'long' });
    return Array.from({ length: 7 }, (_, d) => ({
      value: d as WeekStart,
      label: fmt.format(new Date(2024, 0, 7 + d)),
    }));
  }, [i18n.language]);

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <View style={styles.section}>
        <RadioGroup<LanguageChoice>
          label={t('dialogs.settings.general.languageLabel')}
          labelAsHeading
          value={language}
          options={[
            { value: 'system', label: t('dialogs.settings.general.languageSystem') },
            { value: 'de', label: t('dialogs.settings.general.languageGerman') },
            { value: 'en', label: t('dialogs.settings.general.languageEnglish') },
          ]}
          onChange={onLanguageChange}
        />
      </View>

      <View style={styles.section}>
        <RadioGroup<WeekStart>
          label={t('dialogs.settings.general.weekStartLabel')}
          labelAsHeading
          value={weekStart}
          options={weekdayOptions}
          onChange={onWeekStartChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.general.weekStartHint')}
        </Text>
      </View>

      <View style={styles.section}>
        <RadioGroup<'grid' | 'list'>
          label={t('dialogs.settings.calendars.dayViewMode.heading')}
          labelAsHeading
          value={dayViewMode}
          options={[
            {
              value: 'grid',
              label: t('dialogs.settings.calendars.dayViewMode.options.grid'),
            },
            {
              value: 'list',
              label: t('dialogs.settings.calendars.dayViewMode.options.list'),
            },
          ]}
          onChange={onDayViewModeChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.calendars.dayViewMode.hint')}
        </Text>
      </View>

      <View style={styles.section}>
        <RadioGroup<ThemeModeChoice>
          label={t('dialogs.settings.general.themeLabel')}
          labelAsHeading
          value={themeMode}
          options={[
            { value: 'system', label: t('dialogs.settings.general.themeSystem') },
            { value: 'light', label: t('dialogs.settings.general.themeLight') },
            { value: 'dark', label: t('dialogs.settings.general.themeDark') },
          ]}
          onChange={setThemeMode}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.general.themeHint')}
        </Text>
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.notifications.heading')}
        </Text>
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.notifications.hint')}
        </Text>
        {!globalSound.loading && (
          <SoundSelect
            label={t('dialogs.settings.notifications.globalLabel')}
            value={globalSound.value}
            allowInherit={false}
            onChange={(next) => void globalSound.save(next)}
          />
        )}
        <SwitchRow
          label={t('dialogs.settings.general.hapticsLabel')}
          hint={t('dialogs.settings.general.hapticsHint')}
          value={haptics}
          onToggle={() => setHaptics(!haptics)}
        />
        <SwitchRow
          label={t('dialogs.settings.general.appBadgeLabel')}
          hint={t('dialogs.settings.general.appBadgeHint')}
          value={appBadge}
          onToggle={() => setAppBadge(!appBadge)}
        />
      </View>

      <View style={styles.section}>
        <Text style={styles.heading} accessibilityRole="header">
          {t('dialogs.settings.general.backgroundSyncHeading')}
        </Text>
        <SwitchRow
          label={t('dialogs.settings.general.backgroundSyncLabel')}
          hint={t('dialogs.settings.general.backgroundSyncHint')}
          value={bgSync}
          onToggle={() => setBgSync(!bgSync)}
        />
      </View>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 24 },
    section: { gap: 8 },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
  });
