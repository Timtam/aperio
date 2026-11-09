import { useCallback, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  clearContactsCache,
  setContactsIncludeReadOnlyOnSync,
} from '../api/client';
import { useContactSync } from '../state/useContactSync';
import { useGoToSettingsTab } from '../state/useGoToSettingsTab';

/**
 * Contacts settings panel (DESIGN.md §10.6).
 *
 * Three concerns:
 *
 *   1. **Privacy notice.** Standing reference text explaining what
 *      gets cached locally and pointing at each connected provider's
 *      privacy policy. Always visible in this panel; the one-shot
 *      modal on first-connect (`PrivacyNoticeModal`) is a separate
 *      surface for the same content.
 *
 *   2. **Refresh interval.** None of its own: a sentence and a
 *      button to Settings → General, where the one refresh
 *      interval (`sync.intervalMinutes`) lives
 *      (`useGoToSettingsTab('general')`).
 *
 *   3. **Cache management.** "Cache leeren" wipes every external
 *      adapter's in-memory contact snapshot + resets the persisted
 *      "last synced" timestamp. Local contact rows are user data
 *      (not a cache) and never get touched. A subsequent "Jetzt
 *      synchronisieren" rebuilds the snapshots from each provider.
 *
 * Layout follows the existing panel siblings (`TasksPanel`,
 * `CalendarsPanel`): one `<section>` per concern, each with its
 * own heading + descriptive hint + one or two controls. The
 * Settings dialog wraps the whole panel in a `role="tabpanel"`.
 */

export function ContactsPanel() {
  const { t, i18n } = useTranslation();
  const { status, triggerSync } = useContactSync();
  const announce = useAnnouncer();
  const goToGeneral = useGoToSettingsTab('general');

  const privacyHeadingId = useId();
  const privacyHintId = useId();
  const syncHeadingId = useId();
  const syncHintId = useId();
  const cacheHeadingId = useId();
  const cacheHintId = useId();

  const [clearing, setClearing] = useState(false);
  // Persisted "also pull read-only directories" toggle.
  // Source of truth is the backend's
  // `contacts.includeReadOnlyOnSync` pref, surfaced via
  // `status.include_read_only_on_sync`. Local mirror lets the
  // checkbox feel snappy — flip the UI immediately, persist in
  // the background, reconcile against the next status fetch.
  //
  // Both manual ("Jetzt synchronisieren") and periodic sync
  // passes honour this pref now (the backend reads it on every
  // tick), so the checkbox really is a setting, not a one-shot
  // override.
  const includeDirectoriesId = useId();
  const [includeDirectories, setIncludeDirectories] = useState<boolean | null>(
    null,
  );
  // Seed from status whenever the backend value changes. The
  // hook's polling cycle keeps this fresh; we only overwrite the
  // local draft when the user hasn't touched it, otherwise an
  // in-flight setContactsIncludeReadOnlyOnSync would lose to the
  // stale status response.
  useEffect(() => {
    if (status && includeDirectories === null) {
      setIncludeDirectories(status.include_read_only_on_sync);
    }
  }, [status, includeDirectories]);
  const includeDirectoriesEffective =
    includeDirectories ?? status?.include_read_only_on_sync ?? false;

  const onIncludeDirectoriesToggle = useCallback(
    async (next: boolean) => {
      // Optimistic flip so the checkbox feels instant.
      setIncludeDirectories(next);
      try {
        await setContactsIncludeReadOnlyOnSync(next);
        announce(
          next
            ? t('dialogs.settings.contacts.includeDirectoriesEnabled')
            : t('dialogs.settings.contacts.includeDirectoriesDisabled'),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set_contacts_include_read_only_on_sync failed', err);
        // Roll back so the checkbox reflects persisted state.
        setIncludeDirectories(!next);
      }
    },
    [announce, t],
  );

  const onClearCache = useCallback(async () => {
    if (clearing) return;
    setClearing(true);
    try {
      const succeeded = await clearContactsCache();
      announce(
        t('dialogs.settings.contacts.cacheCleared', { count: succeeded }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('clear_contacts_cache failed', err);
      announce(t('dialogs.settings.contacts.cacheClearFailed'));
    } finally {
      setClearing(false);
    }
  }, [announce, clearing, t]);

  const onSyncNow = useCallback(
    async (includeReadOnly: boolean) => {
      // The button stays focusable while a pass reads the contacts (every
      // refresh does), so the guard lives here (aria-disabled below).
      if (status?.in_flight === true) return;
      await triggerSync(includeReadOnly);
      announce(
        includeReadOnly
          ? t('dialogs.settings.contacts.syncStartedFull')
          : t('dialogs.settings.contacts.syncStarted'),
      );
    },
    [announce, status?.in_flight, t, triggerSync],
  );

  return (
    <div className="form">
      <section
        aria-labelledby={privacyHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={privacyHeadingId} className="color-labels__heading">
          {t('dialogs.settings.contacts.privacyTitle')}
        </h3>
        {/* Informational prose. The Modal wraps the dialog body
            in `role="application"` to keep NVDA in focus mode
            (so form fields behave like form fields). Static
            `<p>` paragraphs would be invisible to focus-mode
            tab navigation; `FocusableNote` makes each one a
            tab stop AND carries the text as `aria-label` so
            NVDA actually reads it. See FocusableNote.tsx for
            why `tabIndex={0}` alone isn't enough. */}
        <FocusableNote id={privacyHintId} className="tasks-settings__hint">
          {t('dialogs.settings.contacts.privacyBody')}
        </FocusableNote>
        <FocusableNote className="tasks-settings__hint">
          {t('dialogs.settings.contacts.privacyProvidersIntro')}
        </FocusableNote>
        {/* Provider privacy-policy links. Open in the system
            browser via Tauri's default click behaviour (the app
            shell intercepts <a target="_blank"> hrefs); each link
            carries an `aria-label` that includes the provider
            name so SR users hear what they're following. */}
        <ul className="tasks-settings__list">
          <li>
            <a
              href="https://policies.google.com/privacy"
              target="_blank"
              rel="noreferrer noopener"
              aria-label={t(
                'dialogs.settings.contacts.providerPolicyAria',
                { provider: 'Google' },
              )}
            >
              {t('dialogs.settings.contacts.providerPolicyGoogle')}
            </a>
          </li>
          <li>
            <a
              href="https://privacy.microsoft.com/privacystatement"
              target="_blank"
              rel="noreferrer noopener"
              aria-label={t(
                'dialogs.settings.contacts.providerPolicyAria',
                { provider: 'Microsoft' },
              )}
            >
              {t('dialogs.settings.contacts.providerPolicyMicrosoft')}
            </a>
          </li>
          <li>
            {/* Inline focusable note. Same pattern as
                `FocusableNote` (text repeated as `aria-label` so
                NVDA in focus mode reads it) but on a `<span>` to
                preserve the inline list-item layout — a `<p>`
                here would break out into block layout. */}
            <span
              className="tasks-settings__hint"
              tabIndex={0}
              aria-label={t('dialogs.settings.contacts.providerPolicyOthers')}
            >
              {t('dialogs.settings.contacts.providerPolicyOthers')}
            </span>
          </li>
        </ul>
      </section>

      <section
        aria-labelledby={syncHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={syncHeadingId} className="color-labels__heading">
          {t('dialogs.settings.contacts.syncTitle')}
        </h3>
        {/* Focus stops: inside the dialog's role="application" plain text
            is not reachable, and this paragraph no longer describes a
            control. How often contacts refresh is the one refresh interval,
            under Allgemein (Toni, 2026-11-09). */}
        <FocusableNote id={syncHintId} className="tasks-settings__hint">
          {t('dialogs.settings.contacts.syncBody')}
        </FocusableNote>
        <FocusableNote className="tasks-settings__hint">
          {t('dialogs.settings.contacts.intervalMoved')}
        </FocusableNote>
        <div className="tasks-settings__row">
          <button type="button" onClick={goToGeneral}>
            {t('dialogs.settings.openGeneral')}
          </button>
        </div>
        {/* The directory toggle is a checkbox because the wording
            describes a state, not an action. Both manual and
            periodic syncs honour it now — the backend reads
            `contacts.includeReadOnlyOnSync` on every tick, so a
            flip here applies on the next round. Pulling
            directories takes minutes on large tenants; the
            unchecked default keeps quiet accounts cheap. */}
        <div className="tasks-settings__row">
          <label htmlFor={includeDirectoriesId} className="form__checkbox">
            <input
              id={includeDirectoriesId}
              type="checkbox"
              checked={includeDirectoriesEffective}
              onChange={(e) => {
                void onIncludeDirectoriesToggle(e.target.checked);
              }}
            />{' '}
            {t('dialogs.settings.contacts.includeDirectoriesLabel')}
          </label>
          <p className="tasks-settings__hint">
            {t('dialogs.settings.contacts.includeDirectoriesHint')}
          </p>
        </div>
        <div className="tasks-settings__row">
          <button
            type="button"
            className="form__action"
            onClick={() => {
              // Pass the effective UI value as an explicit
              // override so a flip + immediate click never races
              // with the in-flight persist. The backend would
              // otherwise re-read the pref before the write had
              // committed.
              void onSyncNow(includeDirectoriesEffective);
            }}
            // Not native `disabled`: every refresh now sets in_flight, and
            // a native disabled button drops the focus of whoever is on it.
            aria-disabled={status?.in_flight === true || undefined}
          >
            {status?.in_flight
              ? t('dialogs.settings.contacts.syncing')
              : t('dialogs.settings.contacts.syncNow')}
          </button>
        </div>
        {/* No `aria-live` here — opening the tab made screen
            readers announce "Last synced at …" out of nowhere,
            which users reported as disorienting. Explicit
            announcements for the actual sync events go through
            `announce()` in the click handlers instead.
            `FocusableNote` makes the paragraph a focus stop +
            sets `aria-label` to the text so NVDA in focus mode
            (the Modal wraps the body in `role="application"`)
            can reach + read it. */}
        <FocusableNote className="tasks-settings__hint">
          {status?.last_synced_at
            ? t('dialogs.settings.contacts.lastSynced', {
                time: new Date(status.last_synced_at).toLocaleString(
                  i18n.language,
                  { dateStyle: 'long', timeStyle: 'short' },
                ),
              })
            : t('dialogs.settings.contacts.neverSynced')}
        </FocusableNote>
      </section>

      <section
        aria-labelledby={cacheHeadingId}
        className="tasks-settings__section"
      >
        <h3 id={cacheHeadingId} className="color-labels__heading">
          {t('dialogs.settings.contacts.cacheTitle')}
        </h3>
        <p id={cacheHintId} className="tasks-settings__hint">
          {t('dialogs.settings.contacts.cacheBody')}
        </p>
        <button
          type="button"
          className="form__action form__action--destructive"
          onClick={() => {
            void onClearCache();
          }}
          disabled={clearing}
          // `aria-describedby` only references the cache-specific
          // hint. We used to also reference `privacyHintId` so the
          // user heard the privacy implications before the
          // destructive action, but that prose is a FocusableNote
          // now (tab-stop with its own aria-label) — re-reading it
          // on every button focus felt repetitive to NVDA users.
          aria-describedby={cacheHintId}
        >
          {clearing
            ? t('dialogs.settings.contacts.clearing')
            : t('dialogs.settings.contacts.clearCache')}
        </button>
      </section>
    </div>
  );
}
