import { useEffect, useState } from 'react';

/**
 * Show a loading indicator only when the wait is long enough to be
 * worth interrupting the user — keeps fast view switches silent.
 *
 * The naive `loading && <Spinner />` pattern flickers a "Lädt …"
 * every time the user switches calendar views (each switch mounts a
 * fresh screen), even though the warm host cache answers in well
 * under 100 ms. Adding a small delay before flipping the indicator
 * on means (the desktop hook of the same name, ported verbatim):
 *
 *   - Fast fetches (local data): the timer never fires, the
 *     indicator never appears, the view feels instant.
 *   - Slow fetches (CalDAV / iCal cold cache, 1–3 s): the timer
 *     fires and the user sees that work is happening — that's the
 *     case we actually wanted to surface.
 *
 * The default 200 ms is the conventional sweet spot — above
 * "instantaneous" but below "noticeably delayed" (Material Design
 * uses 150–200, NN/g treats sub-100 as instant and full-second as
 * noticeable, Apple's HIG suggests waiting roughly a second for
 * a spinner).
 *
 * Behaviour on transient drops: if `loading` toggles false within
 * the delay window, the timer is cancelled and `show` stays false
 * — no flash. The cleanup runs again on the next true to start a
 * fresh window.
 */
export function useDeferredLoading(
  loading: boolean,
  delayMs = 200,
): boolean {
  const [show, setShow] = useState(false);

  useEffect(() => {
    if (!loading) {
      setShow(false);
      return;
    }
    const timer = setTimeout(() => setShow(true), delayMs);
    return () => clearTimeout(timer);
  }, [loading, delayMs]);

  return show;
}
