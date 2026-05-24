import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * "Update required" dialog (DESIGN.md §19.13).
 *
 * The backend latches `SyncStatus.schema_too_old` when a sync
 * round hits a dataset whose `min_app_version` exceeds this
 * build's version. `useSync` watches that flag and pops this
 * dialog once per session via the DialogState.
 *
 * Per §19.13 the dialog is **non-dismissible** — only the two
 * action buttons close it. The wrapping `<Modal>` accepts a
 * `dismissOnBackdrop={false}` so backdrop clicks don't slip
 * through; the parent (`DialogHost`) supplies an `onClose` that
 * we intentionally don't wire to a close button.
 *
 *   - "Jetzt aktualisieren" opens the project's releases page in
 *     the user's default browser. Aperio doesn't have an in-app
 *     updater yet (§21) — this is the bridge until then.
 *   - "Offline fortfahren" closes the modal locally. The backend's
 *     latched state stays in place; the status indicator keeps
 *     showing the warning tone. The user can keep using local
 *     data; no further sync rounds touch the remote until they
 *     update.
 */
export interface SyncSchemaTooOldDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Minimum app version the dataset requires. */
  required: string;
  /** Running app version. Empty string means "skip rendering"
   *  — the backend doesn't surface it; the frontend doesn't
   *  trivially have it either (the version lives behind a Tauri
   *  command we'd otherwise need to thread through useSync).
   *  For v1 we just print `required` and trust the user to know
   *  their own version. */
  running?: string;
}

const RELEASES_URL = 'https://github.com/Timtam/Aperio/releases';

export function SyncSchemaTooOldDialog({
  isOpen,
  onClose,
  required,
  running,
}: SyncSchemaTooOldDialogProps) {
  const { t } = useTranslation();

  const onUpdate = () => {
    // Open the releases page in the OS default browser. Tauri's
    // `opener` plugin would be more idiomatic but `window.open`
    // with a target works from the webview against the configured
    // shell allowlist. Defer until DialogHost wires the opener.
    window.open(RELEASES_URL, '_blank');
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('syncSchemaTooOld.title')}
      className="sync-schema-too-old"
      dismissOnBackdrop={false}
    >
      <p>{t('syncSchemaTooOld.body')}</p>
      <dl className="sync-schema-too-old__versions">
        <dt>{t('syncSchemaTooOld.minVersion')}</dt>
        <dd>{required}</dd>
        {running && (
          <>
            <dt>{t('syncSchemaTooOld.runningVersion')}</dt>
            <dd>{running}</dd>
          </>
        )}
      </dl>
      <p className="sync-schema-too-old__hint">
        {t('syncSchemaTooOld.hint')}
      </p>
      <div className="sync-schema-too-old__actions">
        <button type="button" onClick={onUpdate}>
          {t('syncSchemaTooOld.actionUpdate')}
        </button>
        <button type="button" onClick={onClose}>
          {t('syncSchemaTooOld.actionOffline')}
        </button>
      </div>
    </Modal>
  );
}
