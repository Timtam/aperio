import { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo } from 'react-native';
import { useTranslation } from 'react-i18next';

import { refreshErrors, type AccountRefreshErrors } from '../api/sync';
import { subscribeCacheRefreshProgress } from './cacheRefreshProgress';

/**
 * Per-account refresh-error surface — the mobile twin of the desktop
 * useRefreshErrors: a container whose background refresh keeps failing
 * (revoked provider password, dead server) used to keep serving stale
 * cached rows with no cue anywhere. Reads the aggregate on mount and
 * re-reads whenever an external warm pass ENDS (the moment errors
 * appear or clear).
 *
 * Screen-reader-first: when the affected-account set GROWS, ONE polite
 * announcement names the failure — not per fetch, not per account, and
 * never on clearing (the visible rows disappearing is enough; silence
 * is the healthy state and must stay silent).
 */
export function useRefreshErrors(): {
  /** account_id → its failing containers. Empty map = all healthy. */
  errorsByAccount: Map<string, AccountRefreshErrors>;
} {
  const { t } = useTranslation();
  const [errors, setErrors] = useState<AccountRefreshErrors[]>([]);

  useEffect(() => {
    let cancelled = false;
    let knownAccounts = new Set<string>();

    const refetch = () => {
      refreshErrors()
        .then((rows) => {
          if (cancelled) return;
          setErrors(rows);
          const nowAffected = new Set(rows.map((r) => r.account_id));
          const grew = [...nowAffected].some((id) => !knownAccounts.has(id));
          if (grew) {
            AccessibilityInfo.announceForAccessibility(
              t(
                rows.some((r) => r.auth_suspected)
                  ? 'refreshErrors.announceAuth'
                  : 'refreshErrors.announce',
              ),
            );
          }
          knownAccounts = nowAffected;
        })
        .catch((err) => {
          console.warn('refreshErrors failed', err);
        });
    };

    refetch();
    const unsub = subscribeCacheRefreshProgress((p) => {
      if (!p.refreshing) refetch();
    });
    return () => {
      cancelled = true;
      unsub();
    };
  }, [t]);

  const errorsByAccount = useMemo(() => {
    const map = new Map<string, AccountRefreshErrors>();
    errors.forEach((e) => map.set(e.account_id, e));
    return map;
  }, [errors]);

  return { errorsByAccount };
}
