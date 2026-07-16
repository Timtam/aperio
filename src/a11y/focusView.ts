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

/**
 * Move focus into the active view's stable container: its first focusable
 * descendant (the `role="grid"/"listbox"/"tree"` widget, `tabIndex=0`), else the
 * `[data-active-view-root]` wrapper itself (which carries `tabIndex=-1` for
 * exactly this case). This keeps focus inside `#app-root`'s `role="application"`
 * so the screen reader stays in application mode instead of being stranded on
 * `<body>`. Returns `true` if it focused something, `false` if no view is active.
 */
export function focusActiveView(): boolean {
  const root = document.querySelector('[data-active-view-root]');
  if (!(root instanceof HTMLElement)) return false;
  (findFirstFocusable(root) ?? root).focus({ preventScroll: true });
  return true;
}
