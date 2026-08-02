import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text } from 'react-native';

import {
  applyLanguageChoice,
  readLanguageChoice,
  writeLanguageChoice,
  type LanguageChoice,
} from '../settings/language';
import { useThemedStyles, type ThemeColors } from '../theme';
import { AppDialog } from './AppDialog';
import { RadioGroup } from './RadioGroup';
import {
  SyncTargetConfigForm,
} from './sync/SyncTargetConfigForm';

interface FirstLaunchWizardModalProps {
  visible: boolean;
  onClose: () => void;
}

type WizardStep = 'language' | 'sync' | 'account';

const LANGUAGE_OPTIONS: readonly LanguageChoice[] = ['system', 'de', 'en'];

/**
 * First-launch wizard (DESIGN.md §19.11) — the mobile twin of the desktop
 * `FirstLaunchWizardDialog`. A lean, sync-first, accessible multi-step dialog
 * shown once on a fresh instance:
 *
 *   1. **Language** — reuse the synced `locale` pref (default = system).
 *   2. **Sync** — restore from / create / skip a sync target via the shared
 *      [`SyncTargetConfigForm`](./sync/SyncTargetConfigForm.tsx). RESTORING an
 *      existing dataset brings back data + accounts, so the wizard ENDS;
 *      CREATING a fresh one (or skipping) continues to the account step.
 *   3. **First account** — point the user at Settings → Accounts, or finish.
 *
 * Gated by [`FirstLaunchWizardGate`](./FirstLaunchWizardGate.tsx) so it only
 * ever appears on a genuinely fresh instance.
 */
export function FirstLaunchWizardModal({
  visible,
  onClose,
}: FirstLaunchWizardModalProps) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [step, setStep] = useState<WizardStep>('language');
  const [language, setLanguage] = useState<LanguageChoice>('system');

  useEffect(() => {
    let cancelled = false;
    void readLanguageChoice().then((choice) => {
      if (!cancelled) setLanguage(choice);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const onPickLanguage = useCallback(async (choice: LanguageChoice) => {
    setLanguage(choice);
    try {
      // Persist (synced pref) + apply now so the wizard re-renders in the
      // chosen language.
      await writeLanguageChoice(choice);
      await applyLanguageChoice(choice);
    } catch {
      // Best-effort; the radio still reflects the pick this session.
    }
  }, []);

  const languageOptions = LANGUAGE_OPTIONS.map((opt) => ({
    value: opt,
    label: t(`dialogs.firstLaunchWizard.language_${opt}`),
  }));

  // Storage is the last step either way now — joining brings the accounts down
  // with the dataset, and starting fresh has already been through the account
  // step — so a connected target ends the wizard.
  const onSyncConnected = useCallback(() => {
    onClose();
  }, [onClose]);

  const stepNumber = step === 'language' ? 1 : step === 'account' ? 2 : 3;

  return (
    <AppDialog
      visible={visible}
      title={t('dialogs.firstLaunchWizard.title')}
      cancelLabel={t('mobile.cancel')}
      onCancel={onClose}
    >
      <Text style={styles.stepIndicator} accessibilityRole="text">
        {t('dialogs.firstLaunchWizard.stepIndicator', {
          current: stepNumber,
          total: 3,
        })}
      </Text>

      {step === 'language' && (
        <>
          <Text style={styles.heading} accessibilityRole="header">
            {t('dialogs.firstLaunchWizard.languageHeading')}
          </Text>
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.firstLaunchWizard.languageHint')}
          </Text>
          <RadioGroup<LanguageChoice>
            label={t('dialogs.firstLaunchWizard.languageLegend')}
            value={language}
            options={languageOptions}
            onChange={(c) => void onPickLanguage(c)}
          />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.firstLaunchWizard.next')}
            onPress={() => setStep('account')}
            style={({ pressed }) => [styles.primaryButton, pressed && styles.pressed]}
          >
            <Text style={styles.primaryButtonText}>
              {t('dialogs.firstLaunchWizard.next')}
            </Text>
          </Pressable>
        </>
      )}

      {step === 'sync' && (
        <>
          <Text style={styles.heading} accessibilityRole="header">
            {t('dialogs.firstLaunchWizard.syncHeading')}
          </Text>
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.firstLaunchWizard.syncHint')}
          </Text>
          <SyncTargetConfigForm onConnected={onSyncConnected} />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.firstLaunchWizard.back')}
            onPress={() => setStep('account')}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.firstLaunchWizard.back')}
            </Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.firstLaunchWizard.syncSkip')}
            onPress={onClose}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.firstLaunchWizard.syncSkip')}
            </Text>
          </Pressable>
        </>
      )}

      {step === 'account' && (
        <>
          <Text style={styles.heading} accessibilityRole="header">
            {t('dialogs.firstLaunchWizard.accountHeading')}
          </Text>
          <Text style={styles.hint} accessibilityRole="text">
            {t('dialogs.firstLaunchWizard.accountHint')}
          </Text>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.firstLaunchWizard.back')}
            onPress={() => setStep('language')}
            style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
          >
            <Text style={styles.ghostButtonText}>
              {t('dialogs.firstLaunchWizard.back')}
            </Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.firstLaunchWizard.next')}
            onPress={() => setStep('sync')}
            style={({ pressed }) => [styles.primaryButton, pressed && styles.pressed]}
          >
            <Text style={styles.primaryButtonText}>
              {t('dialogs.firstLaunchWizard.next')}
            </Text>
          </Pressable>
        </>
      )}
    </AppDialog>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    stepIndicator: { fontSize: 13, color: c.textSecondary },
    heading: { fontSize: 17, fontWeight: '700', color: c.textPrimary },
    hint: { fontSize: 13, color: c.textSecondary },
    primaryButton: {
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
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
    pressed: { opacity: 0.7 },
  });
