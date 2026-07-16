import { useEffect } from 'react';

import { findFirstFocusable } from '../a11y/focusView';

/**
 * F6 cycles focus between the major regions of the app shell.
 *
 * DESIGN.md section 3.4 lists `F6` as the way to hop between Sidebar
 * ↔ Toolbar ↔ active view — the same convention native Windows and
 * macOS apps use. Each major region tags itself with
 * `data-region="<name>"`; pressing F6 moves focus to the first
 * tabbable descendant of the next tagged region in DOM order, falling
 * back to the region wrapper itself if it has nothing focusable
 * inside (the active view wrapper carries `tabIndex=-1` for exactly
 * this case). Shift+F6 cycles the other way.
 *
 * Current cycle:
 *   Sidebar → Toolbar → Active view → [Backlog rail, in week/month] → (wrap)
 *
 * Regions are discovered live on every keypress, so a future surface
 * just needs to mount with `data-region` to join the cycle — the backlog
 * rail does exactly that in the week and month views.
 *
 * Regions inside an `inert` subtree are skipped: a collapsed sidebar stays
 * mounted (with `data-region`) but marks itself `inert`, so F6 must hop over
 * it rather than dead-ending on a region that can't take focus.
 */
export function useRegionFocus(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'F6') return;
      e.preventDefault();
      const regions = Array.from(
        document.querySelectorAll<HTMLElement>('[data-region]'),
      ).filter((el) => !el.closest('[inert]'));
      if (regions.length === 0) return;

      const activeRegion = (e.target as HTMLElement | null)?.closest(
        '[data-region]',
      ) as HTMLElement | null;

      let index = activeRegion ? regions.indexOf(activeRegion) : -1;
      const step = e.shiftKey ? -1 : 1;
      index = (index + step + regions.length) % regions.length;

      const next = regions[index];
      const firstFocusable = findFirstFocusable(next);
      (firstFocusable ?? next).focus();
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
}
