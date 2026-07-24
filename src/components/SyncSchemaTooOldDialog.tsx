import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
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
  const introRef = useRef<HTMLParagraphElement>(null);

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
      // Open focus lands on the explanation, NOT the "Jetzt aktualisieren"
      // button — a reflexive Enter must not launch a browser before the user
      // has heard what the dialog wants.
      initialFocusRef={introRef}
    >
      {/*
        Body, versions and hint must be REACHABLE: Modal's body is
        role="application", where static <p>/<dt>/<dd> is invisible to NVDA's
        focus-mode traversal — the user would hear only the title and the two
        buttons, never the required/running versions or that "Offline
        fortfahren" exists. FocusableNote makes the prose focus stops; each
        version value rides on a focusable span labelled "label: value".
      */}
      <FocusableNote ref={introRef}>{t('syncSchemaTooOld.body')}</FocusableNote>
      <dl className="sync-schema-too-old__versions">
        <dt>{t('syncSchemaTooOld.minVersion')}</dt>
        <dd>
          <span
            tabIndex={0}
            aria-label={`${t('syncSchemaTooOld.minVersion')}: ${required}`}
          >
            {required}
          </span>
        </dd>
        {running && (
          <>
            <dt>{t('syncSchemaTooOld.runningVersion')}</dt>
            <dd>
              <span
                tabIndex={0}
                aria-label={`${t('syncSchemaTooOld.runningVersion')}: ${running}`}
              >
                {running}
              </span>
            </dd>
          </>
        )}
      </dl>
      <FocusableNote className="sync-schema-too-old__hint">
        {t('syncSchemaTooOld.hint')}
      </FocusableNote>
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
