import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import { useDialogState } from '../state/DialogState';
import { useSync } from '../state/useSync';

/** Empty string when the running version isn't surfaced — useSync
 *  doesn't have it. The dialog body handles the empty case. */
const RUNNING_VERSION_PLACEHOLDER = '';

/**
 * Persistent sync status badge (DESIGN.md §19.9).
 *
 * Renders one of the four design-spec states:
 *
 *   ✓ Synchronisiert       — idle, last round succeeded
 *   ↑ Wird hochgeladen …   — in_flight
 *   ⚠ Konflikt              — unresolved conflicts present
 *   ✗ Keine Verbindung      — last round errored
 *   ◌ Sync deaktiviert      — no adapter configured
 *
 * Conflicts trump in_flight trump connection error trump synced —
 * conflicts need user input, so the user sees them first even if a
 * sync round is running in the background.
 *
 * The badge is a real `<button>` so keyboard focus + Enter/Space
 * work for free. Click opens the conflicts dialog when conflicts
 * exist, otherwise the Settings → Synchronisation tab.
 *
 * State transitions are announced via the global aria-live polite
 * region so screen-reader users hear "Sync complete" without
 * having to read the badge. Conflict appearance fires the
 * assertive channel because §19.9 specifies system notification +
 * announcement for conflicts.
 */
export function SyncStatusIndicator() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { openSettings, openSyncConflicts, openSyncSchemaTooOld } = useDialogState();
  const { status, lastError, conflictCount } = useSync();

  // Pick the highest-priority state token. Order is deliberate:
  //   schema_too_old > conflicts > error > in_flight > synced > off
  // — schema_too_old blocks sync entirely so it dominates;
  //   conflicts + errors need the user's attention more than a
  //   benign "currently syncing" or "all good" badge.
  const tone:
    | 'schema_too_old'
    | 'conflict'
    | 'error'
    | 'uploading'
    | 'synced'
    | 'off' = (() => {
    if (status?.schema_too_old) return 'schema_too_old';
    if (conflictCount > 0) return 'conflict';
    if (lastError) return 'error';
    if (status?.in_flight) return 'uploading';
    if (status?.configured) return 'synced';
    return 'off';
  })();

  // Glyphs from §19.9. Stored as plain text + aria-hidden so screen
  // readers consume the localised label, not the symbol.
  const glyph: Record<typeof tone, string> = {
    schema_too_old: '⬆',
    conflict: '⚠',
    error: '✗',
    uploading: '↑',
    synced: '✓',
    off: '◌',
  };

  const label: Record<typeof tone, string> = {
    schema_too_old: t('syncStatus.schemaTooOld'),
    conflict:
      conflictCount === 1
        ? t('syncStatus.conflict_one')
        : t('syncStatus.conflict_other', { count: conflictCount }),
    error: t('syncStatus.noConnection'),
    uploading: t('syncStatus.uploading'),
    synced: t('syncStatus.synced'),
    off: t('syncStatus.off'),
  };

  // Announce state transitions. We track the previous tone in a ref
  // and only fire when it changes — without this, every render of
  // the parent re-announces the same text.
  const prevTone = useRef<typeof tone | null>(null);
  useEffect(() => {
    if (prevTone.current === tone) return;
    prevTone.current = tone;
    if (tone === 'schema_too_old') {
      announce(t('syncStatus.announceSchemaTooOld'), 'assertive');
    } else if (tone === 'conflict') {
      // Assertive — the user needs to know now, conflicts block
      // cross-device convergence.
      const message =
        conflictCount === 1
          ? t('syncStatus.announceConflict_one')
          : t('syncStatus.announceConflict_other', { count: conflictCount });
      announce(message, 'assertive');
    } else if (tone === 'error') {
      announce(
        t('syncStatus.announceFailure', { message: lastError ?? '' }),
        'assertive',
      );
    } else if (tone === 'uploading') {
      announce(t('syncStatus.announceUploading'));
    } else if (tone === 'synced') {
      announce(t('syncStatus.announceSynced'));
    }
    // `off` is the no-adapter steady state; no announcement.
  }, [tone, conflictCount, lastError, announce, t]);

  // Clicking the badge routes by tone: schema_too_old → update
  // modal, conflicts → conflicts dialog, anything else → Sync
  // settings tab.
  const onClick = () => {
    if (tone === 'schema_too_old') {
      openSyncSchemaTooOld(
        status?.min_app_version_required ?? '',
        RUNNING_VERSION_PLACEHOLDER,
      );
    } else if (tone === 'conflict') {
      openSyncConflicts();
    } else {
      openSettings('sync');
    }
  };

  const aria =
    tone === 'schema_too_old'
      ? t('syncStatus.openUpdateRequired')
      : tone === 'conflict'
        ? t('syncStatus.openConflicts')
        : t('syncStatus.openSettings');

  return (
    <button
      type="button"
      className={`sync-status sync-status--${tone}`}
      data-tone={tone}
      onClick={onClick}
      aria-label={`${t('syncStatus.label')}: ${label[tone]}. ${aria}.`}
      title={label[tone]}
    >
      <span aria-hidden="true" className="sync-status__glyph">
        {glyph[tone]}
      </span>
      <span className="sync-status__label">{label[tone]}</span>
    </button>
  );
}
