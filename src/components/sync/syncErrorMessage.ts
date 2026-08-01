import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';

import { isCommandError } from '../../api/client';

/**
 * Map a backend `CommandError` (or any thrown value) to a localized,
 * user-facing sync message, keyed on the stable error `code` with a
 * fall-through to the raw message so context is never silently swallowed.
 *
 * Shared by [`SyncTargetConfigForm`](./SyncTargetConfigForm.tsx) and the
 * Settings → Sync E2E sections (`SyncPanel`) so both render identical wording.
 */
export function useSyncErrorMessage(): (err: unknown) => string {
  const { t } = useTranslation();
  return useCallback(
    (err: unknown): string => {
      if (isCommandError(err)) {
        switch (err.code) {
          case 'auth':
            // SFTP host-key mismatch is surfaced as `auth` with a distinctive
            // message prefix — promote it to the dedicated §19.5 warning.
            if (err.message.includes('host key mismatch')) {
              return t('dialogs.settings.sync.adapterSftpHostKeyMismatch');
            }
            return t('dialogs.settings.sync.errorAuth');
          case 'io':
            return t('dialogs.settings.sync.errorIo');
          case 'network':
            return t('dialogs.settings.sync.errorNetwork');
          case 'not_found':
            return t('dialogs.settings.sync.errorNotFound');
          case 'encryption_required':
            return t('dialogs.settings.sync.errorEncryption');
          case 'decryption_failed':
            return t('dialogs.settings.sync.errorDecryption');
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
}
