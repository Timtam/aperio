import { useCallback, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  acceptRemoteDataset,
  adoptLocalDataset,
  compactNow,
  configureSyncAdapter,
  isCommandError,
  previewSyncTarget,
  setSyncInterval,
  type SyncAdapterConfig,
  type SyncPreview,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/DialogState';
import { useSync } from '../state/useSync';

/**
 * Settings → Synchronisation panel (DESIGN.md §19, Phase Si).
 *
 * Four sections:
 *
 *   1. **State** — connection state + last successful sync + manual
 *      `Sync now` / `Compact now`.
 *   2. **Interval** — slider/select over a preset list, defaulting
 *      to 5 minutes per §19.8.
 *   3. **Adapter** — kind picker + path input + Connect / Disconnect.
 *   4. **Onboarding** — preview button + accept/adopt buttons when
 *      a preview has run. Only visible while no adapter is
 *      configured (otherwise the user is already onboarded).
 *
 * The conflicts dialog isn't rendered inline — it's a separate
 * modal reachable from the status badge + a button at the bottom.
 */

const INTERVAL_PRESETS: readonly number[] = [1, 5, 15, 30, 60, 240];

export function SyncPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const { openSyncConflicts } = useDialogState();
  const {
    status,
    lastReport,
    lastError,
    conflictCount,
    triggering,
    triggerSync,
  } = useSync();

  const stateHeadingId = useId();
  const adapterHeadingId = useId();
  const intervalHeadingId = useId();
  const previewHeadingId = useId();

  // Adapter draft state. Seeded from current backend state on mount
  // so the inputs reflect the persisted choice.
  const [kindDraft, setKindDraft] = useState<'local' | 'none'>('local');
  const [pathDraft, setPathDraft] = useState('');
  const [deviceNameDraft, setDeviceNameDraft] = useState('');
  const [intervalDraft, setIntervalDraft] = useState<number | null>(null);
  const [busyAdapter, setBusyAdapter] = useState(false);
  const [busyPreview, setBusyPreview] = useState(false);
  const [busyAccept, setBusyAccept] = useState(false);
  const [busyAdopt, setBusyAdopt] = useState(false);
  const [busyCompact, setBusyCompact] = useState(false);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  const interval = intervalDraft ?? status?.interval_minutes ?? 5;

  // Translate a `CommandError`-shaped error into a frontend message
  // keyed off the stable `code`. Falls back to the raw message when
  // the code is unknown so we never silently swallow context.
  const messageForError = useCallback(
    (err: unknown): string => {
      if (isCommandError(err)) {
        switch (err.code) {
          case 'auth':
            return t('dialogs.settings.sync.errorAuth');
          case 'io':
            return t('dialogs.settings.sync.errorIo');
          case 'network':
            return t('dialogs.settings.sync.errorNetwork');
          case 'not_found':
            return t('dialogs.settings.sync.errorNotFound');
          case 'encryption_required':
            return t('dialogs.settings.sync.errorEncryption');
          case 'schema_too_old':
            return t('dialogs.settings.sync.errorSchemaTooOld');
          default:
            return err.message;
        }
      }
      return err instanceof Error ? err.message : String(err);
    },
    [t],
  );

  const buildConfig = useCallback((): SyncAdapterConfig => {
    if (kindDraft === 'local') return { kind: 'local', path: pathDraft.trim() };
    return { kind: 'none' };
  }, [kindDraft, pathDraft]);

  const onIntervalChange = useCallback(
    async (raw: string) => {
      const parsed = Number(raw);
      if (!Number.isFinite(parsed) || parsed < 1) return;
      setIntervalDraft(parsed);
      try {
        const persisted = await setSyncInterval(parsed);
        setIntervalDraft(persisted);
        announce(
          t('dialogs.settings.sync.intervalChanged', { minutes: persisted }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set_sync_interval failed', err);
        // Restore the previous value so the select doesn't lie.
        setIntervalDraft(status?.interval_minutes ?? 5);
      }
    },
    [announce, status?.interval_minutes, t],
  );

  const onConfigure = useCallback(async () => {
    if (kindDraft === 'local' && !pathDraft.trim()) {
      announce(t('dialogs.settings.sync.adapterNeedPath'), 'assertive');
      return;
    }
    setBusyAdapter(true);
    try {
      await configureSyncAdapter(buildConfig());
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('configure_sync_adapter failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyAdapter(false);
    }
  }, [announce, buildConfig, kindDraft, messageForError, pathDraft, t]);

  const onDisconnect = useCallback(async () => {
    setBusyAdapter(true);
    try {
      await configureSyncAdapter({ kind: 'none' });
      setPreview(null);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('configure_sync_adapter(none) failed', err);
    } finally {
      setBusyAdapter(false);
    }
  }, []);

  const onPreview = useCallback(async () => {
    if (kindDraft !== 'local' || !pathDraft.trim()) {
      announce(t('dialogs.settings.sync.adapterNeedPath'), 'assertive');
      return;
    }
    setBusyPreview(true);
    setPreviewError(null);
    try {
      const result = await previewSyncTarget(buildConfig());
      setPreview(result);
    } catch (err) {
      setPreviewError(messageForError(err));
      setPreview(null);
    } finally {
      setBusyPreview(false);
    }
  }, [announce, buildConfig, kindDraft, messageForError, pathDraft, t]);

  const onAccept = useCallback(async () => {
    setBusyAccept(true);
    try {
      const report = await acceptRemoteDataset(
        buildConfig(),
        deviceNameDraft.trim() || null,
      );
      const message =
        report.device_count === 1
          ? t('dialogs.settings.sync.onboardingDone_one')
          : t('dialogs.settings.sync.onboardingDone_other', {
              count: report.device_count,
            });
      announce(message);
      setPreview(null);
    } catch (err) {
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyAccept(false);
    }
  }, [announce, buildConfig, deviceNameDraft, messageForError, t]);

  const onAdopt = useCallback(async () => {
    const confirmed = window.confirm(t('dialogs.settings.sync.previewAdoptConfirm'));
    if (!confirmed) return;
    setBusyAdopt(true);
    try {
      const report = await adoptLocalDataset(
        buildConfig(),
        deviceNameDraft.trim() || null,
      );
      const message = report.remote_was_empty
        ? t('dialogs.settings.sync.onboardingFresh')
        : t('dialogs.settings.sync.onboardingDone_one');
      announce(message);
      setPreview(null);
    } catch (err) {
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyAdopt(false);
    }
  }, [announce, buildConfig, deviceNameDraft, messageForError, t]);

  const onCompact = useCallback(async () => {
    setBusyCompact(true);
    try {
      const report = await compactNow();
      announce(
        t('dialogs.settings.sync.compactDone', {
          deleted: report.deleted_logs,
        }),
      );
    } catch (err) {
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyCompact(false);
    }
  }, [announce, messageForError, t]);

  const lastSyncedLabel = (() => {
    if (!status?.last_synced_at) return t('dialogs.settings.sync.stateNeverSynced');
    try {
      const dt = new Date(status.last_synced_at);
      return t('dialogs.settings.sync.stateLastSynced', {
        time: fmt.format(dt, 'PPpp'),
      });
    } catch {
      return status.last_synced_at;
    }
  })();

  // When the adapter becomes (un)configured externally — via a
  // second app instance, or the onboarding sub-flow above — keep
  // the preview clean. A configured adapter doesn't need an
  // onboarding card.
  useEffect(() => {
    if (status?.configured) {
      setPreview(null);
    }
  }, [status?.configured]);

  return (
    <div className="sync-panel">
      <section aria-labelledby={stateHeadingId}>
        <h3 id={stateHeadingId}>{t('dialogs.settings.sync.stateTitle')}</h3>
        <p>
          {status?.configured
            ? t('dialogs.settings.sync.stateConfigured', { path: pathDraft || '–' })
            : t('dialogs.settings.sync.stateUnconfigured')}
        </p>
        <p>{lastSyncedLabel}</p>
        {lastReport && (
          <p>
            {t('dialogs.settings.sync.lastReport', {
              pushed: lastReport.pushed_logs,
              fetched: lastReport.fetched_logs,
              applied: lastReport.applied,
            })}
          </p>
        )}
        {lastError && (
          <p className="sync-panel__error" role="alert">
            {t('dialogs.settings.sync.errorPrefix')}: {lastError}
          </p>
        )}
        <div className="sync-panel__actions">
          <button
            type="button"
            disabled={!status?.configured || triggering || status?.in_flight}
            onClick={() => void triggerSync()}
          >
            {status?.in_flight || triggering
              ? t('dialogs.settings.sync.syncing')
              : t('dialogs.settings.sync.syncNow')}
          </button>
          <button
            type="button"
            disabled={!status?.configured || busyCompact}
            onClick={() => void onCompact()}
          >
            {busyCompact
              ? t('dialogs.settings.sync.compacting')
              : t('dialogs.settings.sync.compactNow')}
          </button>
          {conflictCount > 0 && (
            <button type="button" onClick={openSyncConflicts}>
              {conflictCount === 1
                ? t('syncStatus.conflict_one')
                : t('syncStatus.conflict_other', { count: conflictCount })}
            </button>
          )}
        </div>
      </section>

      <section aria-labelledby={intervalHeadingId}>
        <h3 id={intervalHeadingId}>
          {t('dialogs.settings.sync.intervalLabel')}
        </h3>
        <select
          value={interval}
          onChange={(e) => void onIntervalChange(e.target.value)}
          disabled={!status?.configured}
        >
          {INTERVAL_PRESETS.map((min) => (
            <option key={min} value={min}>
              {min === 1
                ? t('dialogs.settings.sync.intervalOption_one', { minutes: min })
                : t('dialogs.settings.sync.intervalOption_other', { minutes: min })}
            </option>
          ))}
        </select>
      </section>

      <section aria-labelledby={adapterHeadingId}>
        <h3 id={adapterHeadingId}>{t('dialogs.settings.sync.adapterTitle')}</h3>
        <p>{t('dialogs.settings.sync.adapterBody')}</p>
        <div className="sync-panel__field">
          <label>
            {t('dialogs.settings.sync.adapterKind')}
            <select
              value={kindDraft}
              onChange={(e) =>
                setKindDraft(e.target.value as 'local' | 'none')
              }
            >
              <option value="local">
                {t('dialogs.settings.sync.adapterKindLocal')}
              </option>
              <option value="none">
                {t('dialogs.settings.sync.adapterKindNone')}
              </option>
            </select>
          </label>
        </div>
        {kindDraft === 'local' && (
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.adapterPath')}
              <input
                type="text"
                value={pathDraft}
                onChange={(e) => setPathDraft(e.target.value)}
                placeholder="/Volumes/NAS/aperio"
              />
            </label>
            <p className="sync-panel__hint">
              {t('dialogs.settings.sync.adapterPathHint')}
            </p>
          </div>
        )}
        <div className="sync-panel__actions">
          <button
            type="button"
            disabled={busyAdapter}
            onClick={() => void onConfigure()}
          >
            {busyAdapter
              ? t('dialogs.settings.sync.adapterConnecting')
              : t('dialogs.settings.sync.adapterConfigure')}
          </button>
          {status?.configured && (
            <button
              type="button"
              disabled={busyAdapter}
              onClick={() => void onDisconnect()}
            >
              {t('dialogs.settings.sync.adapterDisconnect')}
            </button>
          )}
        </div>
      </section>

      {!status?.configured && (
        <section aria-labelledby={previewHeadingId}>
          <h3 id={previewHeadingId}>
            {t('dialogs.settings.sync.previewTitle')}
          </h3>
          <p>{t('dialogs.settings.sync.previewBody')}</p>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.deviceName')}
              <input
                type="text"
                value={deviceNameDraft}
                onChange={(e) => setDeviceNameDraft(e.target.value)}
                placeholder="Desktop"
              />
            </label>
            <p className="sync-panel__hint">
              {t('dialogs.settings.sync.deviceNameHint')}
            </p>
          </div>
          <button
            type="button"
            disabled={busyPreview}
            onClick={() => void onPreview()}
          >
            {t('dialogs.settings.sync.previewButton')}
          </button>
          {previewError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {previewError}
            </p>
          )}
          {preview?.kind === 'empty' && (
            <PreviewEmpty
              t={t}
              busyAdopt={busyAdopt}
              onAdopt={onAdopt}
            />
          )}
          {preview?.kind === 'existing' && (
            <PreviewExisting
              preview={preview}
              t={t}
              fmt={fmt}
              busyAccept={busyAccept}
              busyAdopt={busyAdopt}
              onAccept={onAccept}
              onAdopt={onAdopt}
            />
          )}
        </section>
      )}
    </div>
  );
}

function PreviewEmpty({
  t,
  busyAdopt,
  onAdopt,
}: {
  t: ReturnType<typeof useTranslation>['t'];
  busyAdopt: boolean;
  onAdopt: () => void;
}) {
  return (
    <div className="sync-panel__preview">
      <p>{t('dialogs.settings.sync.previewEmpty')}</p>
      <button
        type="button"
        disabled={busyAdopt}
        onClick={onAdopt}
      >
        {t('dialogs.settings.sync.previewAdoptButton')}
      </button>
    </div>
  );
}

function PreviewExisting({
  preview,
  t,
  fmt,
  busyAccept,
  busyAdopt,
  onAccept,
  onAdopt,
}: {
  preview: Extract<SyncPreview, { kind: 'existing' }>;
  t: ReturnType<typeof useTranslation>['t'];
  fmt: ReturnType<typeof useDateFormat>;
  busyAccept: boolean;
  busyAdopt: boolean;
  onAccept: () => void;
  onAdopt: () => void;
}) {
  const summary =
    preview.snapshot_timestamp !== null
      ? t('dialogs.settings.sync.previewExisting', {
          time: (() => {
            try {
              return fmt.format(
                new Date(preview.snapshot_timestamp as string),
                'PP',
              );
            } catch {
              return preview.snapshot_timestamp;
            }
          })(),
        })
      : t('dialogs.settings.sync.previewNeverCompacted');
  const names = preview.devices
    .map((d) =>
      d.is_this_device
        ? `${d.name ?? d.id} (${t('dialogs.settings.sync.previewThisDevice')})`
        : (d.name ?? d.id),
    )
    .join(', ');
  return (
    <div className="sync-panel__preview">
      <p>{summary}</p>
      <p>
        {t('dialogs.settings.sync.previewDevices', {
          count: preview.devices.length,
          names,
        })}
      </p>
      <div className="sync-panel__preview-actions">
        <div>
          <h4>{t('dialogs.settings.sync.previewAcceptTitle')}</h4>
          <p>{t('dialogs.settings.sync.previewAcceptBody')}</p>
          <button
            type="button"
            disabled={busyAccept}
            onClick={onAccept}
          >
            {t('dialogs.settings.sync.previewAcceptButton')}
          </button>
        </div>
        <div>
          <h4>{t('dialogs.settings.sync.previewAdoptTitle')}</h4>
          <p>{t('dialogs.settings.sync.previewAdoptBody')}</p>
          <button
            type="button"
            disabled={busyAdopt}
            onClick={onAdopt}
          >
            {t('dialogs.settings.sync.previewAdoptButton')}
          </button>
        </div>
      </div>
    </div>
  );
}
