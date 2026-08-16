import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { useCacheRefresh } from '../state/useCacheRefresh';

/**
 * Compact toolbar control for the external-adapter snapshot cache
 * (CACHE-3). Shows a spinning glyph while a background warm pass runs
 * and doubles as a manual "refresh now" button when idle. The tooltip
 * surfaces when external data was last refreshed, so the user knows the
 * snapshot they see may be slightly behind the provider.
 *
 * Distinct from `SyncStatusIndicator` (cross-device event-log sync) —
 * this is about how fresh the *external provider* mirror is.
 */
export function CacheRefreshIndicator() {
  const { t, i18n } = useTranslation();
  const announce = useAnnouncer();
  const { refreshing, lastRefreshedAt, fetchedTargets, totalTargets, refreshNow } =
    useCacheRefresh();

  // Whether the running pass is one the USER started. Background warm passes
  // run on their own all the time; announcing those would be chatter about
  // something nobody asked for.
  const mineRef = useRef(false);
  useEffect(() => {
    if (refreshing || !mineRef.current) return;
    mineRef.current = false;
    // Focus stayed on the button (see below), but a changed accessible name
    // under a focused element is not re-read — so without this the pass ends
    // in silence and the user has no way to know it is done.
    announce(t('cacheRefresh.done'));
  }, [refreshing, announce, t]);

  const lastLabel = lastRefreshedAt
    ? t('cacheRefresh.lastUpdated', {
        time: new Date(lastRefreshedAt).toLocaleString(i18n.language, {
          dateStyle: 'long',
          timeStyle: 'short',
        }),
      })
    : t('cacheRefresh.never');

  // While a pass runs, prefer the live "fetched X of N" once the total is known
  // (the aria-label + tooltip then carry the progress; no aria-live chatter — a
  // screen-reader user reads the current count when they land on the button).
  const refreshingLabel =
    fetchedTargets != null && totalTargets != null
      ? t('cacheRefresh.progress', {
          fetched: fetchedTargets,
          total: totalTargets,
        })
      : t('cacheRefresh.refreshing');

  const title = refreshing ? refreshingLabel : lastLabel;

  return (
    <button
      type="button"
      className={`cache-refresh${refreshing ? ' cache-refresh--spinning' : ''}`}
      onClick={() => {
        // Guarded rather than `disabled`: a native disabled button is removed
        // from the focus order THE INSTANT it flips, and the browser drops
        // focus to <body> rather than moving it anywhere. Pressing this used
        // to strand the screen reader in nothing for the whole pass — and the
        // press is what sets `refreshing`, so it stranded itself.
        if (refreshing) return;
        void (async () => {
          // The command only QUEUES the pass, so it resolves long before the
          // pass ends — arming after it means the completion announcement is
          // armed only once we know there is a pass to complete, with no race
          // against the effect below.
          if (await refreshNow()) mineRef.current = true;
          // Rejected outright. The spinner going out looks exactly like
          // success from here, so announcing "refreshed" would be a lie told
          // in the one place the user is listening.
          else announce(t('cacheRefresh.failed'));
        })();
      }}
      aria-disabled={refreshing || undefined}
      aria-label={`${t('cacheRefresh.label')}: ${title}`}
      title={title}
    >
      <span aria-hidden="true" className="cache-refresh__glyph">
        ⟳
      </span>
    </button>
  );
}
