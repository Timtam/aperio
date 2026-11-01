import { useEffect, useRef, type RefObject } from 'react';

import { useAnnouncer } from '../a11y/announcerContext';
import { useViewState } from '../state/viewStateContext';

export interface PageTurnOptions {
  /**
   * The widget's entries for the new period are still loading. With the
   * focus in the widget the view then says the period itself once they have
   * settled, if nothing in the list will (the day view's empty day).
   */
  pending?: boolean;
  /**
   * A focused control outside the widget that renames itself with the
   * period says enough: the month's and the agenda's check-in button names
   * the day, and with it the month. The week view's does not name the week,
   * so the week view still announces.
   */
  focusNameSays?: boolean;
}

/** What has the focus, and the name NVDA reads for it. In a grid or list
 *  the focus stays on the widget and names its active cell or entry; a
 *  widget with no active entry has nothing to read (`name` null). */
function focusSnapshot(widget: HTMLElement | null): {
  focused: Element | null;
  name: string | null;
} {
  const focused = document.activeElement;
  if (!focused || focused === document.body) return { focused: null, name: null };
  const activeId = focused.getAttribute('aria-activedescendant');
  const named = activeId
    ? document.getElementById(activeId)
    : focused === widget
      ? null
      : focused;
  const name = named
    ? `${activeId ?? ''}\n${named.getAttribute('aria-label') ?? named.textContent ?? ''}`
    : null;
  return { focused, name };
}

/**
 * Say the period a view shows after the user asked for another one (Back,
 * Today, Forward, Ctrl+←/→, Ctrl+T; TODO B15 step 3), unless what has the
 * focus says it.
 *
 * NVDA reads the name of what has the focus when that name changes: the
 * active cell or entry of a grid or list, or a focused button. So the view
 * stays quiet exactly when the page turn renamed it — the new cell names its
 * date, the day view's entry the new day — and announces when nothing was
 * renamed: the focus on a toolbar button, a list with no entry, an agenda
 * whose first entry stays the same, Today pressed on today. Judged from what
 * is on screen rather than from the anchor, because a moved anchor need not
 * rename anything.
 *
 * `spoken` is the period in words, as the view shows it now; `widget` the
 * element with the focus model (grid or list).
 */
export function usePageTurnAnnouncement(
  spoken: string,
  widget: RefObject<HTMLElement>,
  { pending = false, focusNameSays = false }: PageTurnOptions = {},
): void {
  const { pageTurn } = useViewState();
  const announce = useAnnouncer();
  const last = useRef<{ pageTurn: number } & ReturnType<typeof focusSnapshot>>({
    pageTurn,
    focused: null,
    name: null,
  });

  // A focus move renders nothing (Tab into the grid, say), so the picture
  // the next page turn is compared with is taken on each move as well.
  useEffect(() => {
    const onFocusIn = () => {
      last.current = { ...last.current, ...focusSnapshot(widget.current) };
    };
    document.addEventListener('focusin', onFocusIn);
    return () => document.removeEventListener('focusin', onFocusIn);
  }, [widget]);

  useEffect(() => {
    const before = last.current;
    const now = focusSnapshot(widget.current);
    last.current = { pageTurn, ...now };
    if (pageTurn === before.pageTurn) return;
    const renamed =
      now.focused !== null &&
      now.focused === before.focused &&
      now.name !== null &&
      now.name !== before.name;
    const inWidget = widget.current?.contains(now.focused) ?? false;
    if (inWidget && (renamed || pending)) return;
    if (!inWidget && renamed && focusNameSays) return;
    announce(spoken);
  });
}
