import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { RootStackScreenProps } from '../navigation/types';
import { RadioGroup } from '../components/RadioGroup';
import { SoundSelect } from '../components/SoundSelect';
import { useSoundPref } from '../state/useSoundPref';
import { useThemedStyles, type ThemeColors } from '../theme';
import {
  applyLanguageChoice,
  readLanguageChoice,
  writeLanguageChoice,
  type LanguageChoice,
} from '../settings/language';
import { readWeekStart, writeWeekStart, type WeekStart } from '../settings/weekStart';

// Settings hub — the app-config home, reachable from the Tasks toolbar. Houses
// the language override (the one piece of real config that has mobile backing
// today) plus links to the connection-management screens (Accounts, Sync) that
// used to clutter the Tasks toolbar. Screen-reader-first: the language picker is
// a labelled radio group; each link is a button announcing its destination.

export default function SettingsScreen({
  navigation,
}: RootStackScreenProps<'Settings'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [language, setLanguage] = useState<LanguageChoice>('system');
  const [weekStart, setWeekStart] = useState<WeekStart>(1);
  // The global default reminder sound (§14.4 root). System/Silent only on mobile
  // — Custom needs an asset store the host lacks; a custom value synced from
  // desktop still round-trips and is shown read-only by SoundSelect.
  const globalSound = useSoundPref('sound.global');

  // Reflect the stored choices whenever the screen is focused (they may have
  // been applied on launch — or changed on another device — before this screen
  // mounted).
  useFocusEffect(
    useCallback(() => {
      void readLanguageChoice().then(setLanguage);
      void readWeekStart().then(setWeekStart);
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
      <RadioGroup<LanguageChoice>
        label={t('dialogs.settings.general.languageLabel')}
        value={language}
        options={[
          { value: 'system', label: t('dialogs.settings.general.languageSystem') },
          { value: 'de', label: t('dialogs.settings.general.languageGerman') },
          { value: 'en', label: t('dialogs.settings.general.languageEnglish') },
        ]}
        onChange={onLanguageChange}
      />

      <View style={styles.section}>
        <RadioGroup<WeekStart>
          label={t('dialogs.settings.general.weekStartLabel')}
          value={weekStart}
          options={weekdayOptions}
          onChange={onWeekStartChange}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.general.weekStartHint')}
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
      </View>

      <View style={styles.links}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.tabs.tasks')}
          onPress={() => navigation.navigate('TaskSettings')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.settings.tabs.tasks')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.accounts.title')}
          onPress={() => navigation.navigate('Accounts')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.accounts.title')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.tabs.contacts')}
          onPress={() => navigation.navigate('ContactSettings')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.settings.tabs.contacts')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.syncTitle')}
          onPress={() => navigation.navigate('Sync')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('mobile.syncTitle')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.reminders.title')}
          onPress={() => navigation.navigate('Reminders')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.reminders.title')}</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.colorLabels.title')}
          onPress={() => navigation.navigate('ColorLabels')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.colorLabels.title')}</Text>
        </Pressable>
      </View>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 20 },
    section: { gap: 8 },
    heading: { fontSize: 17, fontWeight: '700', color: c.textLabel },
    hint: { fontSize: 14, color: c.textSecondary, lineHeight: 20 },
    links: { gap: 12 },
    link: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    linkText: { fontSize: 16, fontWeight: '600', color: c.link },
    pressed: { opacity: 0.7 },
  });
