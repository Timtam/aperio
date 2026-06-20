import { useTranslation } from 'react-i18next';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Settings hub — the app-config home (the SettingsTab stack root). A clean list
// of destinations only: General (language / week start / sounds), Tasks,
// Accounts, Contacts, Sync, Reminders, Colour labels, Logs. The actual controls
// live on the pushed sub-screens, so the hub doesn't mix inline settings with
// links. Screen-reader-first: each row is a button announcing its destination.

export default function SettingsScreen({
  navigation,
}: RootStackScreenProps<'Settings'>) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      <View style={styles.links}>
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.tabs.general')}
          onPress={() => navigation.navigate('General')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.settings.tabs.general')}</Text>
        </Pressable>
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
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.tabs.logs')}
          onPress={() => navigation.navigate('Logs')}
          style={({ pressed }) => [styles.link, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>{t('dialogs.settings.tabs.logs')}</Text>
        </Pressable>
      </View>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 20 },
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
