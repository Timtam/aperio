import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { AdapterKind } from '../api/types';
import { Modal } from './Modal';

/**
 * One-shot privacy notice (DESIGN.md §10.6) shown the first time
 * the user connects an account whose ContactsFeature impl pulls
 * remote address-book data. Acknowledging it writes the
 * `contacts.privacyNoticeAcknowledged` user pref so subsequent
 * connects skip the modal; the same body text remains available
 * as a standing reference in `ContactsPanel`.
 *
 * The notice carries three pieces:
 *
 *   - A short explanation of what gets cached locally (names,
 *     emails, birthdays — derived from the trait surface, not
 *     the literal SQLite schema).
 *   - A provider-specific privacy-policy link, picked off
 *     `adapterKind` so the user gets the one that matters for
 *     the account they just connected.
 *   - A single "Verstanden" / "OK" button. There is no cancel
 *     path — at this point the account has already been
 *     created, so a cancel-shaped action would be misleading.
 *     If the user wants out, they delete the account in the
 *     same panel.
 *
 * The single acknowledge button focuses on mount so a keyboard
 * user can dismiss with Enter without a Tab.
 */

export interface ContactsPrivacyNoticeModalProps {
  isOpen: boolean;
  /** Adapter kind of the account that was just connected.
   *  Decides which provider policy link to surface. `null`
   *  while the modal is closed; the consumer uses it to derive
   *  `isOpen`. */
  adapterKind: AdapterKind | null;
  onAcknowledge: () => void;
}

interface ProviderPolicy {
  name: string;
  url: string;
}

/** Map an adapter kind to the provider whose privacy policy the
 *  notice should link to. CardDAV is generic — the user-supplied
 *  server URL is the source of truth, so we don't presume a
 *  specific policy and just surface the generic line about "your
 *  provider's policy" instead. */
function providerPolicyFor(kind: AdapterKind | null): ProviderPolicy | null {
  switch (kind) {
    case 'google':
      return { name: 'Google', url: 'https://policies.google.com/privacy' };
    case 'microsoft_graph':
      return {
        name: 'Microsoft',
        url: 'https://privacy.microsoft.com/privacystatement',
      };
    case 'ews':
      return {
        name: 'Microsoft',
        url: 'https://privacy.microsoft.com/privacystatement',
      };
    case 'caldav':
      return null;
    default:
      return null;
  }
}

export function ContactsPrivacyNoticeModal({
  isOpen,
  adapterKind,
  onAcknowledge,
}: ContactsPrivacyNoticeModalProps) {
  const { t } = useTranslation();
  const acknowledgeRef = useRef<HTMLButtonElement>(null);

  // Auto-focus the OK button so Enter dismisses without a Tab.
  // queueMicrotask defers past the Modal's own focus trap so the
  // button is the first focusable element after the mount cycle
  // settles.
  useEffect(() => {
    if (!isOpen) return;
    queueMicrotask(() => acknowledgeRef.current?.focus());
  }, [isOpen]);

  const provider = providerPolicyFor(adapterKind);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onAcknowledge}
      title={t('dialogs.accounts.privacyNotice.title')}
      className="modal--confirm"
      dismissOnBackdrop={false}
    >
      <div className="form">
        <p>{t('dialogs.accounts.privacyNotice.body')}</p>
        {provider ? (
          <p>
            {t('dialogs.accounts.privacyNotice.providerLine', {
              provider: provider.name,
            })}{' '}
            <a
              href={provider.url}
              target="_blank"
              rel="noreferrer noopener"
            >
              {t('dialogs.accounts.privacyNotice.providerLink', {
                provider: provider.name,
              })}
            </a>
          </p>
        ) : (
          <p>{t('dialogs.accounts.privacyNotice.providerGeneric')}</p>
        )}
        <p className="form__hint">
          {t('dialogs.accounts.privacyNotice.cacheHint')}
        </p>
      </div>
      <div className="form__actions">
        <button
          ref={acknowledgeRef}
          type="button"
          className="form__action form__action--primary"
          onClick={onAcknowledge}
        >
          {t('dialogs.accounts.privacyNotice.acknowledge')}
        </button>
      </div>
    </Modal>
  );
}
