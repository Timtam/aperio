import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';

import { useAnnouncer } from '../a11y/Announcer';
import {
  clearSyncLog,
  listSyncLogEntries,
  type SyncLogEntry,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';

/**
 * Settings → Synchronisation → Protokoll section
 * (DESIGN.md §19.9 "Detailliertes Sync-Protokoll").
 *
 * Renders the last N (up to backend's 200-row cap) sync rounds
 * newest-first. Each row shows:
 *
 *   - Timestamp (locale-formatted via `useDateFormat`)
 *   - Trigger badge (manual / periodisch / Erstkontakt / …)
 *   - Outcome — counters for success, error string for failure
 *   - Duration in ms
 *
 * Lives outside the global DialogState stack — the parent
 * SyncPanel embeds it inline. Refreshes on:
 *
 *   - mount
 *   - `sync-log-changed` Tauri event (emitted by the scheduler
 *     after every recorded round)
 *
 * "Verlauf leeren" button drops every row via the
 * `clear_sync_log` command. Bracketed by a `window.confirm`
 * because it's irreversible.
 */
export interface SyncProtocolSectionProps {
  /** ID for the section heading — used by the parent's
   *  `aria-labelledby` so screen readers announce "Protokoll"
   *  when focus lands in the section. */
  headingId: string;
}

export function SyncProtocolSection({ headingId }: SyncProtocolSectionProps) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const [entries, setEntries] = useState<SyncLogEntry[]>([]);
  const [busyClear, setBusyClear] = useState(false);

  const refresh = useCallback(() => {
    listSyncLogEntries()
      .then(setEntries)
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('list_sync_log_entries failed', err);
      });
  }, []);

  // Initial load.
  useEffect(() => {
    refresh();
  }, [refresh]);

  // Listen for `sync-log-changed` so the list updates without
  // polling. Cleanup follows the same cancelled+unlisten dance
  // useSync uses.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen('sync-log-changed', () => {
      refresh();
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('sync-log-changed listen failed', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refresh]);

  const onClear = useCallback(async () => {
    const confirmed = window.confirm(
      t('dialogs.settings.sync.protocolClearConfirm'),
    );
    if (!confirmed) return;
    setBusyClear(true);
    try {
      await clearSyncLog();
      setEntries([]);
      announce(t('dialogs.settings.sync.protocolCleared'));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('clear_sync_log failed', err);
    } finally {
      setBusyClear(false);
    }
  }, [announce, t]);

  return (
    <section aria-labelledby={headingId} className="sync-panel__protocol">
      <h3 id={headingId}>{t('dialogs.settings.sync.protocolTitle')}</h3>
      <p>{t('dialogs.settings.sync.protocolBody')}</p>
      {entries.length === 0 ? (
        <p className="sync-panel__hint">
          {t('dialogs.settings.sync.protocolEmpty')}
        </p>
      ) : (
        <ul
          className="sync-panel__protocol-list"
          aria-label={t('dialogs.settings.sync.protocolListLabel')}
        >
          {entries.map((e) => (
            <ProtocolRow key={e.id} entry={e} fmt={fmt} />
          ))}
        </ul>
      )}
      {entries.length > 0 && (
        <div className="sync-panel__actions">
          <button
            type="button"
            disabled={busyClear}
            onClick={() => void onClear()}
          >
            {busyClear
              ? t('dialogs.settings.sync.protocolClearing')
              : t('dialogs.settings.sync.protocolClear')}
          </button>
        </div>
      )}
    </section>
  );
}

/** One row in the protocol list. Extracted so the parent's
 *  render stays light, and so the row's per-entry hooks (the
 *  trigger label resolver) don't leak into the parent's deps. */
function ProtocolRow({
  entry,
  fmt,
}: {
  entry: SyncLogEntry;
  fmt: ReturnType<typeof useDateFormat>;
}) {
  const { t } = useTranslation();
  const triggerLabel = (() => {
    switch (entry.trigger) {
      case 'manual':
        return t('dialogs.settings.sync.protocolTriggerManual');
      case 'periodic':
        return t('dialogs.settings.sync.protocolTriggerPeriodic');
      case 'kick':
        return t('dialogs.settings.sync.protocolTriggerKick');
      case 'app_start':
        return t('dialogs.settings.sync.protocolTriggerAppStart');
      case 'app_exit':
        return t('dialogs.settings.sync.protocolTriggerAppExit');
      default:
        // Forward-compat: an unknown trigger from a newer
        // backend renders verbatim so we don't hide info.
        return entry.trigger;
    }
  })();
  const timestamp = (() => {
    try {
      return fmt.format(new Date(entry.recorded_at), 'PPpp');
    } catch {
      return entry.recorded_at;
    }
  })();
  const summary = entry.success
    ? t('dialogs.settings.sync.protocolSummarySuccess', {
        pushed: entry.pushed_logs ?? 0,
        fetched: entry.fetched_logs ?? 0,
        applied: entry.applied ?? 0,
      })
    : t('dialogs.settings.sync.protocolSummaryFailure', {
        error: entry.error ?? '?',
      });

  return (
    <li
      className={`sync-panel__protocol-row sync-panel__protocol-row--${
        entry.success ? 'ok' : 'fail'
      }`}
    >
      <span aria-hidden="true" className="sync-panel__protocol-glyph">
        {entry.success ? '✓' : '✗'}
      </span>
      <div className="sync-panel__protocol-meta">
        <span className="sync-panel__protocol-time">{timestamp}</span>
        <span className="sync-panel__protocol-trigger">{triggerLabel}</span>
        {typeof entry.duration_ms === 'number' && (
          <span className="sync-panel__protocol-duration">
            {t('dialogs.settings.sync.protocolDuration', {
              ms: entry.duration_ms,
            })}
          </span>
        )}
      </div>
      <p className="sync-panel__protocol-summary">{summary}</p>
    </li>
  );
}
