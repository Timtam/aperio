import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * The stable code a sync failure carries across the FFI boundary.
 *
 * Expo surfaces a `CodedException` (Android) / an `Exception` with a `code`
 * (iOS) as `error.code`, and the native modules re-throw `StoreError::Sync`
 * as one so the engine's own `SyncError::code()` arrives intact. Before that
 * every sync failure reached this side as `StoreError::Storage`, so the phone
 * showed the engine's English `Display` text while the desktop, branching on
 * the same code, showed a translated sentence.
 */
function codeOf(err: unknown): string | null {
  if (err && typeof err === 'object' && 'code' in err) {
    const code = (err as { code?: unknown }).code;
    if (typeof code === 'string' && code.length > 0) return code;
  }
  return null;
}

/**
 * Map a sync failure to a localized sentence, keyed on the code, falling
 * through to the raw message so context is never silently swallowed.
 *
 * The twin of the desktop's `useSyncErrorMessage`, reading the same locale keys
 * on purpose: the two platforms describing one failure differently is a bug the
 * user cannot report, because they only ever see one of them.
 */
export function useSyncErrorMessage(): (err: unknown) => string {
  const { t } = useTranslation();
  return useCallback(
    (err: unknown): string => {
      switch (codeOf(err)) {
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
        case 'decryption_failed':
          return t('dialogs.settings.sync.errorDecryption');
        case 'schema_too_old':
          return t('dialogs.settings.sync.errorSchemaTooOld');
        default:
          // Everything else — including a `protocol` or `internal` failure the
          // user can do nothing about — keeps the engine's own words. A
          // generic "something went wrong" would be shorter and would cost the
          // one detail that makes a bug report useful.
          return err instanceof Error ? err.message : String(err);
      }
    },
    [t],
  );
}
