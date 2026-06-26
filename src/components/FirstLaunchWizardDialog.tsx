import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import {
  readLanguagePref,
  setLanguagePref,
  type LanguagePref,
} from '../intl/language';
import { useDialogState } from '../state/dialogStateContext';
import { useSync } from '../state/useSync';
import { Modal } from './Modal';
import {
  SyncTargetConfigForm,
  type SyncConnectOutcome,
} from './sync/SyncTargetConfigForm';

interface FirstLaunchWizardDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

type WizardStep = 'language' | 'sync' | 'account';

const LANGUAGE_OPTIONS: readonly LanguagePref[] = ['system', 'de', 'en'];

/**
 * First-launch wizard (DESIGN.md §19.11). A lean, sync-first, accessible
 * multi-step dialog shown once on a fresh instance:
 *
 *   1. **Language** — reuse the synced `locale` pref (default = system).
 *   2. **Sync** — restore from / create / skip a sync target via the shared
 *      [`SyncTargetConfigForm`](./sync/SyncTargetConfigForm.tsx). RESTORING an
 *      existing dataset brings back data + accounts, so the wizard ENDS;
 *      CREATING a fresh one (or skipping) continues to the account step.
 *   3. **First account** — hand off to the add-account flow, or skip.
 *
 * Gated by [`FirstLaunchWizardChecker`](./FirstLaunchWizardChecker.tsx) so it
 * only ever appears on a genuinely fresh instance.
 */
export function FirstLaunchWizardDialog({
  isOpen,
  onClose,
}: FirstLaunchWizardDialogProps) {
  const { t } = useTranslation();
  const { openAccounts, openSyncAccountsConnect } = useDialogState();
  const { status } = useSync();

  const [step, setStep] = useState<WizardStep>('language');
  const [language, setLanguage] = useState<LanguagePref>('system');

  // Seed the radio from the persisted choice.
  useEffect(() => {
    let cancelled = false;
    void readLanguagePref().then((pref) => {
      if (!cancelled) setLanguage(pref);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const onPickLanguage = useCallback(async (pref: LanguagePref) => {
    setLanguage(pref);
    try {
      // Applies immediately via i18next, so the wizard itself re-renders in
      // the chosen language.
      await setLanguagePref(pref);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('setLanguagePref failed', err);
    }
  }, []);

  // The shared form connected. RESTORE (joined an existing dataset) ends the
  // wizard — data + accounts are already back; any missing credentials are
  // handed to the §19.11 reconnect dialog. CREATE (fresh dataset) continues to
  // the account step.
  const onSyncConnected = useCallback(
    (outcome: SyncConnectOutcome) => {
      if (outcome.joined) {
        onClose();
        if (outcome.accountsNeedingConnect.length > 0) {
          openSyncAccountsConnect(outcome.accountsNeedingConnect);
        }
        return;
      }
      setStep('account');
    },
    [onClose, openSyncAccountsConnect],
  );

  const onAddAccount = useCallback(() => {
    onClose();
    openAccounts();
  }, [onClose, openAccounts]);

  const stepNumber = step === 'language' ? 1 : step === 'sync' ? 2 : 3;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.firstLaunchWizard.title')}
      className="modal--first-launch-wizard"
      dismissOnBackdrop={false}
    >
      <div className="first-launch-wizard">
        <FocusableNote className="first-launch-wizard__step-indicator">
          {t('dialogs.firstLaunchWizard.stepIndicator', {
            current: stepNumber,
            total: 3,
          })}
        </FocusableNote>

        {step === 'language' && (
          <section className="first-launch-wizard__step">
            <h3>{t('dialogs.firstLaunchWizard.languageHeading')}</h3>
            <FocusableNote className="first-launch-wizard__hint">
              {t('dialogs.firstLaunchWizard.languageHint')}
            </FocusableNote>
            <fieldset className="first-launch-wizard__radiogroup">
              <legend>{t('dialogs.firstLaunchWizard.languageLegend')}</legend>
              {LANGUAGE_OPTIONS.map((opt) => (
                <label key={opt}>
                  <input
                    type="radio"
                    name="first-launch-language"
                    value={opt}
                    checked={language === opt}
                    onChange={() => void onPickLanguage(opt)}
                  />{' '}
                  {t(`dialogs.firstLaunchWizard.language_${opt}`)}
                </label>
              ))}
            </fieldset>
            <div className="first-launch-wizard__actions">
              <button type="button" onClick={() => setStep('sync')}>
                {t('dialogs.firstLaunchWizard.next')}
              </button>
            </div>
          </section>
        )}

        {step === 'sync' && (
          <section className="first-launch-wizard__step">
            <h3>{t('dialogs.firstLaunchWizard.syncHeading')}</h3>
            <FocusableNote className="first-launch-wizard__hint">
              {t('dialogs.firstLaunchWizard.syncHint')}
            </FocusableNote>
            <SyncTargetConfigForm status={status} onConnected={onSyncConnected} />
            <div className="first-launch-wizard__actions">
              <button type="button" onClick={() => setStep('language')}>
                {t('dialogs.firstLaunchWizard.back')}
              </button>
              <button type="button" onClick={() => setStep('account')}>
                {t('dialogs.firstLaunchWizard.syncSkip')}
              </button>
            </div>
          </section>
        )}

        {step === 'account' && (
          <section className="first-launch-wizard__step">
            <h3>{t('dialogs.firstLaunchWizard.accountHeading')}</h3>
            <FocusableNote className="first-launch-wizard__hint">
              {t('dialogs.firstLaunchWizard.accountHint')}
            </FocusableNote>
            <div className="first-launch-wizard__actions">
              <button type="button" onClick={() => setStep('sync')}>
                {t('dialogs.firstLaunchWizard.back')}
              </button>
              <button type="button" onClick={onAddAccount}>
                {t('dialogs.firstLaunchWizard.accountAdd')}
              </button>
              <button type="button" onClick={onClose}>
                {t('dialogs.firstLaunchWizard.finish')}
              </button>
            </div>
          </section>
        )}
      </div>
    </Modal>
  );
}
