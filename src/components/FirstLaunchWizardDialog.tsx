import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import {
  readLanguagePref,
  setLanguagePref,
  type LanguagePref,
} from '../intl/language';
import { useDialogState } from '../state/dialogStateContext';
import { Modal } from './Modal';
import {
  SyncTargetSchemaForm,
  type SyncConnectOutcome,
} from './sync/SyncTargetSchemaForm';

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
 *   2. **First account** — hand off to the add-account flow, or skip.
 *   3. **Sync** — restore from / create / skip a sync target via
 *      [`SyncTargetSchemaForm`](./sync/SyncTargetSchemaForm.tsx), whose fields
 *      come from the chosen backend's own account schema. Last either way:
 *      restoring brings the accounts down with the dataset, and creating has
 *      already been through the account step.
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

  const [step, setStep] = useState<WizardStep>('language');
  const [language, setLanguage] = useState<LanguagePref>('system');

  // Each step advance/back UNMOUNTS the button that was just pressed (only one
  // step renders at a time), which drops native focus to <body> — outside
  // #app-root's role="application" — so NVDA silently leaves application mode
  // and the wizard's Escape/Tab handlers go dead. Move focus onto the newly
  // mounted step's heading on every transition so focus stays inside the dialog
  // and NVDA announces which step the user landed on. A useLayoutEffect (not
  // useEffect) so it runs synchronously in the same commit. We fire only when
  // the step VALUE actually changed (not merely on every effect run): that
  // leaves Modal's own open-focus (the step indicator) untouched on mount and
  // is robust to StrictMode's dev-only double-invocation. Only the mounted
  // step's <h3> binds headingRef, so this always targets the step now on
  // screen. Also covers the async connect→account jump in onSyncConnected,
  // which has the identical unmount-under-focus shape.
  const headingRef = useRef<HTMLHeadingElement>(null);
  const prevStepRef = useRef(step);
  useLayoutEffect(() => {
    if (prevStepRef.current === step) return;
    prevStepRef.current = step;
    headingRef.current?.focus({ preventScroll: true });
  }, [step]);

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

  // The shared form connected, and storage is the last step either way, so the
  // wizard is done. What differs is only what comes after: a JOINED dataset can
  // carry accounts whose credentials are not on this device, and those go to
  // the §19.11 reconnect dialog.
  const onSyncConnected = useCallback(
    (outcome: SyncConnectOutcome) => {
      onClose();
      if (outcome.joined && outcome.accountsNeedingConnect.length > 0) {
        openSyncAccountsConnect(outcome.accountsNeedingConnect);
      }
    },
    [onClose, openSyncAccountsConnect],
  );

  const onAddAccount = useCallback(() => {
    onClose();
    openAccounts();
  }, [onClose, openAccounts]);

  const stepNumber = step === 'language' ? 1 : step === 'account' ? 2 : 3;

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
            <h3 ref={headingRef} tabIndex={-1}>
              {t('dialogs.firstLaunchWizard.languageHeading')}
            </h3>
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
              <button type="button" onClick={() => setStep('account')}>
                {t('dialogs.firstLaunchWizard.next')}
              </button>
            </div>
          </section>
        )}

        {step === 'sync' && (
          <section className="first-launch-wizard__step">
            <h3 ref={headingRef} tabIndex={-1}>
              {t('dialogs.firstLaunchWizard.syncHeading')}
            </h3>
            <FocusableNote className="first-launch-wizard__hint">
              {t('dialogs.firstLaunchWizard.syncHint')}
            </FocusableNote>
            <SyncTargetSchemaForm onConnected={onSyncConnected} />
            <div className="first-launch-wizard__actions">
              <button type="button" onClick={() => setStep('account')}>
                {t('dialogs.firstLaunchWizard.back')}
              </button>
              {/* Last step now, so skipping storage ends the wizard rather
                  than moving to one the user has already been through. */}
              <button type="button" onClick={onClose}>
                {t('dialogs.firstLaunchWizard.syncSkip')}
              </button>
            </div>
          </section>
        )}

        {step === 'account' && (
          <section className="first-launch-wizard__step">
            <h3 ref={headingRef} tabIndex={-1}>
              {t('dialogs.firstLaunchWizard.accountHeading')}
            </h3>
            <FocusableNote className="first-launch-wizard__hint">
              {t('dialogs.firstLaunchWizard.accountHint')}
            </FocusableNote>
            <div className="first-launch-wizard__actions">
              <button type="button" onClick={() => setStep('language')}>
                {t('dialogs.firstLaunchWizard.back')}
              </button>
              <button type="button" onClick={onAddAccount}>
                {t('dialogs.firstLaunchWizard.accountAdd')}
              </button>
              <button type="button" onClick={() => setStep('sync')}>
                {t('dialogs.firstLaunchWizard.next')}
              </button>
            </div>
          </section>
        )}
      </div>
    </Modal>
  );
}
