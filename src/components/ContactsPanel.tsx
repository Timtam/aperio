import { useCallback, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  clearContactsCache,
  setContactsSyncInterval,
} from '../api/client';
import { useContactSync } from '../state/useContactSync';

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
 *   2. **Sync interval.** Dropdown over a small preset list
 *      (15 / 30 / 60 / 120 / 240 min). The backend clamps
 *      and persists; the scheduler re-reads on every tick, so a
 *      change here applies on the next periodic pass without an
 *      app restart.
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

/** Preset sync-interval offerings (minutes). Anything outside this
 *  list still round-trips through `setContactsSyncInterval` (the
 *  backend clamps to [1, 1440]) — these are just the convenient
 *  values exposed in the UI. */
const INTERVAL_PRESETS: readonly number[] = [15, 30, 60, 120, 240];

export function ContactsPanel() {
  const { t } = useTranslation();
  const { status, triggerSync } = useContactSync();
  const announce = useAnnouncer();

  const privacyHeadingId = useId();
  const privacyHintId = useId();
  const syncHeadingId = useId();
  const syncHintId = useId();
  const intervalLabelId = useId();
  const intervalSelectId = useId();
  const cacheHeadingId = useId();
  const cacheHintId = useId();

  // Local mirror of the interval so the select updates instantly
  // on change; the persisted value comes back through
  // `setContactsSyncInterval` and re-syncs via `useContactSync`'s
  // next status fetch. We seed from `status?.interval_minutes`
  // and fall back to 60 (the same default the backend uses).
  const [intervalDraft, setIntervalDraft] = useState<number | null>(null);
  const interval = intervalDraft ?? status?.interval_minutes ?? 60;

  const [clearing, setClearing] = useState(false);

  const onIntervalChange = useCallback(
    async (raw: string) => {
      const parsed = Number(raw);
      if (!Number.isFinite(parsed) || parsed < 1) return;
      setIntervalDraft(parsed);
      try {
        const persisted = await setContactsSyncInterval(parsed);
        // Reconcile in case the backend clamped — the UI now
        // reflects what's actually stored.
        setIntervalDraft(persisted);
        announce(
          t('dialogs.settings.contacts.intervalChanged', {
            minutes: persisted,
          }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set_contacts_sync_interval failed', err);
        // Roll back the optimistic update so the select shows
        // whatever the persisted value is.
        setIntervalDraft(null);
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
      await triggerSync(includeReadOnly);
      announce(
        includeReadOnly
          ? t('dialogs.settings.contacts.syncStartedFull')
          : t('dialogs.settings.contacts.syncStarted'),
      );
    },
    [announce, t, triggerSync],
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
        {/* The body is split across two paragraphs so the section
            reads like prose rather than a wall of text. Both get
            referenced via `aria-describedby` on the cache button
            below — the user hears the privacy implications before
            committing to the destructive action. */}
        <p id={privacyHintId} className="tasks-settings__hint">
          {t('dialogs.settings.contacts.privacyBody')}
        </p>
        <p className="tasks-settings__hint">
          {t('dialogs.settings.contacts.privacyProvidersIntro')}
        </p>
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
            <span className="tasks-settings__hint">
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
        <p id={syncHintId} className="tasks-settings__hint">
          {t('dialogs.settings.contacts.syncBody')}
        </p>
        <div className="tasks-settings__row">
          <label
            id={intervalLabelId}
            htmlFor={intervalSelectId}
            className="form__label"
          >
            {t('dialogs.settings.contacts.intervalLabel')}
          </label>
          <select
            id={intervalSelectId}
            value={interval}
            aria-labelledby={intervalLabelId}
            aria-describedby={syncHintId}
            onChange={(e) => {
              void onIntervalChange(e.target.value);
            }}
          >
            {INTERVAL_PRESETS.map((m) => (
              <option key={m} value={m}>
                {t('dialogs.settings.contacts.intervalOption', {
                  minutes: m,
                })}
              </option>
            ))}
            {/* If the persisted value isn't one of the presets
                (a power user wrote it manually via user_prefs),
                surface it as an extra option so the select still
                reflects state truthfully. */}
            {!INTERVAL_PRESETS.includes(interval) && (
              <option value={interval}>
                {t('dialogs.settings.contacts.intervalOption', {
                  minutes: interval,
                })}
              </option>
            )}
          </select>
        </div>
        <div className="tasks-settings__row">
          <button
            type="button"
            className="form__action"
            onClick={() => {
              void onSyncNow(false);
            }}
            disabled={status?.in_flight === true}
          >
            {status?.in_flight
              ? t('dialogs.settings.contacts.syncing')
              : t('dialogs.settings.contacts.syncNow')}
          </button>
          <button
            type="button"
            className="form__action"
            onClick={() => {
              void onSyncNow(true);
            }}
            disabled={status?.in_flight === true}
            title={t('dialogs.settings.contacts.syncFullHint')}
          >
            {t('dialogs.settings.contacts.syncFull')}
          </button>
        </div>
        <p className="tasks-settings__hint" aria-live="polite">
          {status?.last_synced_at
            ? t('dialogs.settings.contacts.lastSynced', {
                time: new Date(status.last_synced_at).toLocaleString(),
              })
            : t('dialogs.settings.contacts.neverSynced')}
        </p>
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
          aria-describedby={`${cacheHintId} ${privacyHintId}`}
        >
          {clearing
            ? t('dialogs.settings.contacts.clearing')
            : t('dialogs.settings.contacts.clearCache')}
        </button>
      </section>
    </div>
  );
}
