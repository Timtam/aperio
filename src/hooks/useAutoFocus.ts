import { useEffect, useRef, type RefObject } from 'react';

/**
 * Focus the referenced element once it is "ready".
 *
 * Default behaviour (no argument): focuses on mount, the first chance
 * React has. That's the right choice for views that have no async
 * dependency — YearView, the Modal body, etc.
 *
 * With a `ready` boolean: defers the focus until `ready` first turns
 * `true`. The grids that depend on `useEvents` / `useTasks` pass
 * `!loading` so the focus only lands after the data has arrived. That
 * matters because the cell's `aria-label` carries the event count
 * ("Wednesday, 14 May, 4 events"); focusing before the fetch resolves
 * would have the screen reader announce the *initial* empty state and
 * never re-announce when the real data shows up.
 *
 * `preventScroll: true` stops the page from jumping when the target
 * is off-screen; the view's own scroll handling decides placement.
 *
 * The focus fires at most once per mount — toggling `ready` back to
 * `false` and `true` again will *not* re-focus.
 */
export function useAutoFocus<T extends HTMLElement>(
  ready: boolean = true,
): RefObject<T> {
  const ref = useRef<T>(null);
  const done = useRef(false);

  useEffect(() => {
    if (done.current) return;
    if (!ready) return;
    if (!ref.current) return;
    ref.current.focus({ preventScroll: true });
    done.current = true;
  }, [ready]);

  return ref;
}
