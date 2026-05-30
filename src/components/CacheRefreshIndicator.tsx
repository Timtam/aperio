import { useTranslation } from 'react-i18next';

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
  const { refreshing, lastRefreshedAt, refreshNow } = useCacheRefresh();

  const lastLabel = lastRefreshedAt
    ? t('cacheRefresh.lastUpdated', {
        time: new Date(lastRefreshedAt).toLocaleString(i18n.language),
      })
    : t('cacheRefresh.never');

  const title = refreshing ? t('cacheRefresh.refreshing') : lastLabel;

  return (
    <button
      type="button"
      className={`cache-refresh${refreshing ? ' cache-refresh--spinning' : ''}`}
      onClick={() => void refreshNow()}
      disabled={refreshing}
      aria-label={`${t('cacheRefresh.label')}: ${title}`}
      title={title}
    >
      <span aria-hidden="true" className="cache-refresh__glyph">
        ⟳
      </span>
    </button>
  );
}
