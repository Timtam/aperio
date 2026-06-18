import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { RootStackScreenProps } from '../navigation/types';
import { RadioGroup } from '../components/RadioGroup';
import {
  applyLanguageChoice,
  readLanguageChoice,
  writeLanguageChoice,
  type LanguageChoice,
} from '../settings/language';

// Settings hub — the app-config home, reachable from the Tasks toolbar. Houses
// the language override (the one piece of real config that has mobile backing
// today) plus links to the connection-management screens (Accounts, Sync) that
// used to clutter the Tasks toolbar. Screen-reader-first: the language picker is
// a labelled radio group; each link is a button announcing its destination.

export default function SettingsScreen({
  navigation,
}: RootStackScreenProps<'Settings'>) {
  const { t } = useTranslation();
  const [language, setLanguage] = useState<LanguageChoice>('system');

  // Reflect the stored choice whenever the screen is focused (it may have been
  // applied on launch before this screen mounted).
  useFocusEffect(
    useCallback(() => {
      void readLanguageChoice().then(setLanguage);
    }, []),
  );

  const onLanguageChange = useCallback((next: LanguageChoice) => {
    setLanguage(next);
    void writeLanguageChoice(next);
    void applyLanguageChoice(next);
  }, []);

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

      <View style={styles.links}>
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
          accessibilityLabel={t('mobile.syncTitle')}
          onPress={() => navigation.navigate('Sync')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('mobile.syncTitle')}</Text>
        </Pressable>
      </View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 20 },
  links: { gap: 12 },
  link: {
    paddingVertical: 14,
    paddingHorizontal: 18,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  linkText: { fontSize: 16, fontWeight: '600', color: '#1d3a2f' },
  pressed: { opacity: 0.7 },
});
