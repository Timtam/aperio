import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
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
 * Open focus lands on the notice text (via `initialFocusRef`), not the
 * acknowledge button — the whole point of the modal is that the user reads
 * (hears) what gets cached and whose policy applies before dismissing it.
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
  const introRef = useRef<HTMLParagraphElement>(null);

  const provider = providerPolicyFor(adapterKind);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onAcknowledge}
      title={t('dialogs.accounts.privacyNotice.title')}
      className="modal--confirm"
      dismissOnBackdrop={false}
      initialFocusRef={introRef}
    >
      {/*
        Every line must be REACHABLE — Modal's body is role="application", where
        a static <p> is invisible to NVDA's focus-mode traversal, so the notice
        (what is cached, whose policy applies) was never spoken. FocusableNote
        makes each paragraph a focus stop; the provider line keeps its live link
        (a focus stop already) but its leading text rides a focusable span.
      */}
      <div className="form">
        <FocusableNote ref={introRef}>
          {t('dialogs.accounts.privacyNotice.body')}
        </FocusableNote>
        {provider ? (
          <p>
            <span
              tabIndex={0}
              aria-label={t('dialogs.accounts.privacyNotice.providerLine', {
                provider: provider.name,
              })}
            >
              {t('dialogs.accounts.privacyNotice.providerLine', {
                provider: provider.name,
              })}
            </span>{' '}
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
          <FocusableNote>
            {t('dialogs.accounts.privacyNotice.providerGeneric')}
          </FocusableNote>
        )}
        <FocusableNote className="form__hint">
          {t('dialogs.accounts.privacyNotice.cacheHint')}
        </FocusableNote>
      </div>
      <div className="form__actions">
        <button
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
