import { useEffect } from 'react';

/**
 * F6 cycles focus between the major regions of the app shell.
 *
 * DESIGN.md section 3.4 lists `F6` as the way to hop between Sidebar ↔
 * Calendar ↔ Toolbar — the same convention native Windows and macOS
 * apps use. Each major region tags itself with `data-region="<name>"`;
 * pressing F6 moves focus to the first tabbable descendant of the next
 * tagged region in DOM order.
 *
 * Shift+F6 cycles the other way.
 */
export function useRegionFocus(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'F6') return;
      e.preventDefault();
      const regions = Array.from(
        document.querySelectorAll<HTMLElement>('[data-region]'),
      );
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

function findFirstFocusable(root: HTMLElement): HTMLElement | null {
  // Anything natively focusable that isn't disabled and isn't aria-hidden.
  const candidates = root.querySelectorAll<HTMLElement>(
    'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
  );
  for (const el of candidates) {
    if (el.hasAttribute('disabled')) continue;
    if (el.getAttribute('aria-hidden') === 'true') continue;
    return el;
  }
  return null;
}
