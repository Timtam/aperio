import { useEffect } from 'react';

import { findRegionTarget, regionStops } from '../a11y/focusView';

/**
 * F6 cycles focus between the major regions of the app shell, and so do
 * Ctrl+Tab and Ctrl+Shift+Tab (Toni, 2026-10-31).
 *
 * DESIGN.md section 3.4 lists `F6` as the way to hop between the regions —
 * the same convention native Windows and macOS apps use. Each major region
 * tags itself with `data-region="<name>"`, and lands focus on what it marks
 * with `data-region-target` (else its first tabbable descendant, else the
 * wrapper itself). Shift cycles the other way.
 *
 * The cycle, in this order whatever the DOM order (`regionStops`):
 *   Sidebar → the active view's button → [Backlog's tasks, in week/month]
 *   → the view itself (its grid, list or tree) → (wrap)
 *
 * The check-in button is no stop: Tab reaches it. Regions are discovered
 * live on every keypress, so a future surface just needs to mount with
 * `data-region` to join the cycle. Regions inside an `inert` subtree are
 * skipped: a collapsed sidebar stays mounted but inert, and so is the whole
 * shell while a dialog is open — then the keys are the dialog's.
 */
export function useRegionFocus(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const ctrlTab = e.key === 'Tab' && e.ctrlKey && !e.altKey && !e.metaKey;
      if (e.key !== 'F6' && !ctrlTab) return;
      const target = e.target instanceof Element ? e.target : null;
      // In a dialog Ctrl+Tab moves through it as Tab does.
      if (ctrlTab && target?.closest('[role="dialog"], [role="alertdialog"]')) return;
      e.preventDefault();
      const regions = regionStops();
      if (regions.length === 0) return;

      const activeRegion = target?.closest('[data-region]') as HTMLElement | null;
      let index = activeRegion ? regions.indexOf(activeRegion) : -1;
      const step = e.shiftKey ? -1 : 1;
      // From outside every region, forward starts at the first and back at
      // the last.
      if (index < 0 && step < 0) index = 0;
      index = (index + step + regions.length) % regions.length;

      findRegionTarget(regions[index]).focus();
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
}
