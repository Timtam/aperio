/**
 * Helpers for returning keyboard / screen-reader focus into the active view —
 * the region inside `#app-root`'s `role="application"`. Used whenever a precise
 * focus target is gone or was never captured (a dialog closing back to the view,
 * the calendar-focus exit banner, F6 region cycling), so the "land on something
 * focusable inside the view, else the wrapper" heuristic lives in one place.
 */

/**
 * First natively-focusable descendant of `root` a keyboard user could land on —
 * buttons, links, form controls, and anything with a non-negative tabindex.
 * Skips disabled, aria-hidden, and inert nodes.
 */
export function findFirstFocusable(root: HTMLElement): HTMLElement | null {
  const candidates = root.querySelectorAll<HTMLElement>(
    'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
  );
  for (const el of candidates) {
    if (el.hasAttribute('disabled')) continue;
    if (el.getAttribute('aria-hidden') === 'true') continue;
    if (el.closest('[inert]')) continue;
    return el;
  }
  return null;
}

/** The regions F6 and Ctrl+Tab stop at, in this order (Toni, 2026-10-31):
 *  the backlog before the view, as it stands left of the grid. A region of
 *  another name comes after them, in DOM order. */
const REGION_ORDER = ['sidebar', 'toolbar', 'backlog', 'view'];

/** The regions that can take focus now — none inside an `inert` subtree —
 *  in the order the cycle visits them. */
export function regionStops(): HTMLElement[] {
  const rank = (el: HTMLElement) => {
    const at = REGION_ORDER.indexOf(el.dataset.region ?? '');
    return at < 0 ? REGION_ORDER.length : at;
  };
  return Array.from(document.querySelectorAll<HTMLElement>('[data-region]'))
    .filter((el) => !el.closest('[inert]'))
    .map((el, domIndex) => ({ el, domIndex }))
    .sort((a, b) => rank(a.el) - rank(b.el) || a.domIndex - b.domIndex)
    .map(({ el }) => el);
}

/**
 * Where focus lands in `region`: what it marks with `data-region-target` — the
 * view's own grid, list or tree, the toolbar's active view, the backlog's
 * list — and not one a region inside it marks; else its first focusable
 * descendant; else the region itself.
 */
export function findRegionTarget(region: HTMLElement): HTMLElement {
  const marked = Array.from(
    region.querySelectorAll<HTMLElement>('[data-region-target]'),
  ).find(
    (el) =>
      el.closest('[data-region]') === region &&
      !el.hasAttribute('disabled') &&
      el.getAttribute('aria-hidden') !== 'true' &&
      !el.closest('[inert]'),
  );
  return marked ?? findFirstFocusable(region) ?? region;
}

/**
 * Move focus into the active view's stable container: the widget it marks as
 * its target (the `role="grid"/"listbox"/"tree"`, `tabIndex=0`) — not the
 * check-in button in its header, nor the backlog beside the grid —, else its
 * first focusable descendant, else the `[data-active-view-root]` wrapper
 * itself (which carries `tabIndex=-1` for exactly this case). This keeps focus
 * inside `#app-root`'s `role="application"` so the screen reader stays in
 * application mode instead of being stranded on `<body>`. Returns `true` if it
 * focused something, `false` if no view is active.
 */
export function focusActiveView(): boolean {
  const root = document.querySelector('[data-active-view-root]');
  if (!(root instanceof HTMLElement)) return false;
  findRegionTarget(root).focus({ preventScroll: true });
  return true;
}
