import { useCallback, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import { useViewState } from '../state/viewStateContext';

/**
 * Top-of-main banner shown while the user is "focused" on a single
 * calendar (drill-in from the sidebar context menu).
 *
 * Renders nothing in the default multi-calendar mode. When active:
 *
 *   - shows the focused calendar's display name
 *   - offers an explicit "Exit focus mode" button
 *   - registers a window-level Escape handler that fires only when
 *     no modal is open — modals retain Escape priority for their own
 *     close-on-cancel semantics
 *   - emits an SR announcement on entry and exit so screen-reader
 *     users know the calendar set just shrank / grew
 */
export function FocusBar() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { focusedCalendarId, exitFocus } = useViewState();
  const { calendars } = useCalendarStore();
  const { mode: dialogMode } = useDialogState();

  // Look up the focused calendar's display name. If it's been removed
  // from the catalog while focused (account deleted, sync race), we
  // still render the banner with a fallback string — the next render
  // pass usually catches up.
  const focusedCalendar = focusedCalendarId
    ? calendars.find((c) => c.id === focusedCalendarId)
    : null;
  const displayName = focusedCalendar?.name ?? '';

  // Single exit handler shared by the click button, Escape, and the
  // auto-exit when the focused calendar disappears. Beyond clearing
  // the focused id and announcing, it also moves keyboard / SR focus
  // onto the active view's wrapper — without this, clicking the
  // exit button leaves focus on a node that's about to unmount, and
  // focus falls back to <body>. Screen-reader users would then have
  // no context for where they are.
  const exitAndRestoreFocus = useCallback(() => {
    exitFocus();
    announce(t('sidebar.focus.exitedAnnouncement'));
    // Defer past React's commit so we focus *after* the FocusBar has
    // unmounted; otherwise the focused element is gone before the
    // browser settles on a default fallback.
    requestAnimationFrame(() => {
      const root = document.querySelector(
        '[data-active-view-root]',
      ) as HTMLElement | null;
      if (!root) return;
      // Prefer the first natively-focusable descendant of the view
      // (a button, a tabbable grid cell) so the user lands on
      // something interactive. Fall back to the wrapper itself (it
      // carries tabIndex=-1 for exactly this case).
      const target = findFirstFocusable(root) ?? root;
      target.focus({ preventScroll: true });
    });
  }, [exitFocus, announce, t]);

  // Global Escape handler. Yields to any open modal (a modal's own
  // close-on-Escape would otherwise be eaten). `useEffect` (not
  // `useLayoutEffect`) is correct: the listener attaches after the
  // banner mounts, which is the same tick the focus state flips on.
  const handleEscape = useCallback(
    (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (dialogMode.kind !== 'none') return;
      // Guard against text-input fields swallowing Escape with their
      // own meaning (e.g. clearing a search input).
      const target = e.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName.toLowerCase();
        if (tag === 'input' || tag === 'textarea' || target.isContentEditable) {
          return;
        }
      }
      e.preventDefault();
      exitAndRestoreFocus();
    },
    [dialogMode, exitAndRestoreFocus],
  );

  useEffect(() => {
    if (!focusedCalendarId) return;
    window.addEventListener('keydown', handleEscape);
    return () => window.removeEventListener('keydown', handleEscape);
  }, [focusedCalendarId, handleEscape]);

  // Auto-exit when the focused calendar disappears from the catalog
  // (deleted from another window, account removed, etc.). Guarded by
  // a non-empty catalog check so we don't bounce out during the
  // initial load before calendars finish hydrating.
  useEffect(() => {
    if (!focusedCalendarId) return;
    if (calendars.length === 0) return;
    const stillThere = calendars.some((c) => c.id === focusedCalendarId);
    if (!stillThere) {
      exitAndRestoreFocus();
    }
  }, [focusedCalendarId, calendars, exitAndRestoreFocus]);

  if (!focusedCalendarId) return null;

  return (
    <div className="focus-bar" role="region" aria-label={t('sidebar.focus.banner', { name: displayName })}>
      <span className="focus-bar__icon" aria-hidden="true">
        ●
      </span>
      <span className="focus-bar__label">
        {t('sidebar.focus.banner', { name: displayName })}
      </span>
      <span className="focus-bar__hint" aria-hidden="true">
        {t('sidebar.focus.exitHint')}
      </span>
      <button
        type="button"
        className="focus-bar__exit"
        onClick={exitAndRestoreFocus}
      >
        {t('sidebar.focus.exit')}
      </button>
    </div>
  );
}

/**
 * Find the first natively-focusable descendant of `root`. Matches
 * the same heuristic `useRegionFocus` uses for F6 navigation —
 * buttons, links, form controls, and anything with a non-negative
 * tabindex. Excludes disabled and aria-hidden nodes.
 */
function findFirstFocusable(root: HTMLElement): HTMLElement | null {
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
