import { useCallback, useRef, useState } from 'react';

import { refreshExternalCache, syncNow } from '../api/sync';

/**
 * Native pull-to-refresh for a scroll surface (calendar / tasks / contacts). On
 * pull it kicks the SAME manual update as the header sync button — a peer sync
 * round (if a target is configured) plus an external-cache warm — and reports
 * `refreshing` for a `RefreshControl`. The lists reload themselves off the
 * resulting cache-update / dataVersion signals, so this only awaits the fetch.
 *
 * A11y: RefreshControl (UIRefreshControl) is VoiceOver-accessible — a three-
 * finger swipe DOWN while scrolled to the top triggers it. That works on the
 * tasks + contacts lists. On the CALENDAR surfaces the CalendarPager claims the
 * three-finger swipe natively (`accessibilityScroll:`) to page between periods,
 * so the gesture never reaches this control there; VoiceOver users refresh those
 * via the always-present, focusable header sync button (same action).
 */
export function usePullRefresh(): {
  refreshing: boolean;
  onRefresh: () => void;
} {
  const [refreshing, setRefreshing] = useState(false);
  // Guard against overlapping pulls (a fast second pull while one is in flight).
  const inFlight = useRef(false);

  const onRefresh = useCallback(() => {
    if (inFlight.current) return;
    inFlight.current = true;
    setRefreshing(true);
    void Promise.allSettled([
      syncNow('manual'),
      refreshExternalCache(),
    ]).finally(() => {
      inFlight.current = false;
      setRefreshing(false);
    });
  }, []);

  return { refreshing, onRefresh };
}
