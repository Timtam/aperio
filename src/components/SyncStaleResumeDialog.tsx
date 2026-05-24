import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import { FocusableNote } from '../a11y/FocusableNote';
import { isCommandError, resumeStaleDevice } from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { Modal } from './Modal';

/**
 * "This device was offline for a while" resume dialog
 * (DESIGN.md §19.10).
 *
 * The backend latches `SyncStatus.stale_device_since` when a sync
 * round notices our `meta.devices[me].stale == true`. `useSync`
 * watches that latch and pops this dialog once per latch cycle.
 *
 * Single forward action ("Fortfahren") that triggers the resume
 * command on the backend: re-pull the current snapshot, replay
 * any post-snapshot logs, clear the device's stale flag, advance
 * the cursor. Errors stay visible in the dialog until the user
 * dismisses; the latch persists so the dialog re-opens on the
 * next sync round if they close without resolving.
 *
 * **Local-edit caveat (v1):** `apply_snapshot_dump` is an upsert
 * — rows only in local SQLite (created during the offline
 * window) survive; shared rows get overwritten with the snapshot
 * state, clobbering any local edits made while offline. Pending
 * logs that carry those edits are still on disk and push on the
 * next sync round, so other devices see + merge them. The local
 * UI may briefly show pre-edit values until a follow-up sync
 * round relays the edits back via another device. The dialog
 * body mentions this so the user knows what to expect.
 */
export interface SyncStaleResumeDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** RFC3339 timestamp of the snapshot the dialog should
   *  reference — surfaced so the user understands which point in
   *  time the local data will jump back to. */
  snapshotAt: string;
}

export function SyncStaleResumeDialog({
  isOpen,
  onClose,
  snapshotAt,
}: SyncStaleResumeDialogProps) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const snapshotLabel = (() => {
    try {
      return fmt.format(new Date(snapshotAt), 'PPpp');
    } catch {
      return snapshotAt;
    }
  })();

  const onConfirm = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const report = await resumeStaleDevice();
      announce(
        t('syncStaleResume.doneAnnouncement', {
          applied: report.applied,
        }),
      );
      onClose();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('resume_stale_device failed', err);
      const message = isCommandError(err) ? err.message : String(err);
      setError(message);
    } finally {
      setBusy(false);
    }
  }, [announce, onClose, t]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('syncStaleResume.title')}
      className="sync-stale-resume"
      // Single forward action; users shouldn't dismiss by
      // backdrop click and skip the conscious gesture. They can
      // still Escape if they want to defer.
      dismissOnBackdrop={false}
    >
      <FocusableNote className="sync-stale-resume__body">
        {t('syncStaleResume.body', { time: snapshotLabel })}
      </FocusableNote>
      <FocusableNote className="sync-stale-resume__merge-hint">
        {t('syncStaleResume.mergeHint')}
      </FocusableNote>
      {error && (
        <p className="sync-stale-resume__error" role="alert">
          {t('syncStaleResume.errorPrefix')}: {error}
        </p>
      )}
      <div className="sync-stale-resume__actions">
        <button type="button" onClick={() => void onConfirm()} disabled={busy}>
          {busy
            ? t('syncStaleResume.applying')
            : t('syncStaleResume.actionContinue')}
        </button>
      </div>
    </Modal>
  );
}
