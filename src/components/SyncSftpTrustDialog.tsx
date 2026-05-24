import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { HostKeyPreview } from '../api/client';
import { Modal } from './Modal';

/**
 * SFTP host-key trust dialog (DESIGN.md §19.5).
 *
 * Surfaced by [`SyncPanel`] before it configures an SFTP adapter
 * when the backend's `previewSftpHostKey` reports either:
 *
 *   - `status.kind === 'new'` — first contact with this host:port.
 *     Show the fingerprint + a "verify out-of-band, then accept"
 *     prompt before TOFU pins it. No comparison; just one
 *     fingerprint and an Accept / Cancel pair.
 *
 *   - `status.kind === 'changed'` — known host but the fingerprint
 *     differs from what's pinned. Show both stored + presented
 *     side-by-side with a stronger warning: this could be a key
 *     rotation OR a man-in-the-middle. The user MUST positively
 *     re-pin via Accept; Cancel keeps the old pin in place and
 *     the configure flow refuses the connection.
 *
 * The `status.kind === 'unchanged'` case never reaches the dialog
 * — the parent skips opening it and proceeds straight to
 * configure.
 *
 * Accept calls back into the parent with the freshly-observed
 * fingerprint; the parent calls `trustSftpHostKey` to commit
 * before kicking off the real configure round.
 *
 * Why isn't this in the global DialogState stack? Because the
 * confirmation is part of a single async chain inside SyncPanel
 * (preview → user gesture → configure). Local state keeps the
 * callback path obvious and avoids threading a promise-like
 * resolver through the global state.
 */
export interface SyncSftpTrustDialogProps {
  isOpen: boolean;
  preview: HostKeyPreview | null;
  /** Called when the user accepts. Receives the fingerprint to
   *  pin so the parent can call `trustSftpHostKey` and then
   *  proceed with the configure step. */
  onAccept: (fingerprint: string) => void;
  onCancel: () => void;
}

export function SyncSftpTrustDialog({
  isOpen,
  preview,
  onAccept,
  onCancel,
}: SyncSftpTrustDialogProps) {
  const { t } = useTranslation();
  const [submitting, setSubmitting] = useState(false);

  // Reset the submitting flag when the dialog re-opens for a
  // different preview — otherwise the second open would render
  // with the disabled button stuck from the first round.
  useEffect(() => {
    if (isOpen) setSubmitting(false);
  }, [isOpen, preview?.fingerprint]);

  const handleAccept = useCallback(() => {
    if (!preview) return;
    setSubmitting(true);
    onAccept(preview.fingerprint);
  }, [onAccept, preview]);

  if (!preview) return null;
  const isChange = preview.status.kind === 'changed';
  const title = isChange
    ? t('dialogs.settings.sync.sftpTrustChangedTitle')
    : t('dialogs.settings.sync.sftpTrustNewTitle');

  return (
    <Modal
      isOpen={isOpen}
      onClose={onCancel}
      title={title}
      className="modal--sftp-trust"
      // Either action is a deliberate gesture; backdrop dismiss is
      // fine — it maps to Cancel, same as Escape.
      dismissOnBackdrop
    >
      <div className="sftp-trust">
        <p>
          {isChange
            ? t('dialogs.settings.sync.sftpTrustChangedBody')
            : t('dialogs.settings.sync.sftpTrustNewBody')}
        </p>
        <dl className="sftp-trust__details">
          <dt>{t('dialogs.settings.sync.sftpTrustHostLabel')}</dt>
          <dd>
            <code>{preview.host_port}</code>
          </dd>
          {isChange && preview.status.kind === 'changed' && (
            <>
              <dt>{t('dialogs.settings.sync.sftpTrustStoredLabel')}</dt>
              <dd>
                <code>{preview.status.stored}</code>
              </dd>
            </>
          )}
          <dt>{t('dialogs.settings.sync.sftpTrustPresentedLabel')}</dt>
          <dd>
            <code>{preview.fingerprint}</code>
          </dd>
        </dl>
        <p className="sftp-trust__verify">
          {t('dialogs.settings.sync.sftpTrustVerifyHint')}
        </p>
        <div className="sftp-trust__actions">
          <button type="button" onClick={onCancel}>
            {t('dialogs.settings.sync.sftpTrustCancel')}
          </button>
          <button
            type="button"
            onClick={handleAccept}
            disabled={submitting}
            className={
              isChange ? 'sftp-trust__accept--changed' : 'sftp-trust__accept'
            }
          >
            {isChange
              ? t('dialogs.settings.sync.sftpTrustAcceptChanged')
              : t('dialogs.settings.sync.sftpTrustAcceptNew')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
