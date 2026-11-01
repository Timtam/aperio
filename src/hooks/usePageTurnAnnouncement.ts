import { useEffect, useRef, type RefObject } from 'react';

import { useAnnouncer } from '../a11y/announcerContext';
import { useViewState } from '../state/viewStateContext';

/**
 * Say the period a view shows after the user asked for another one (Back,
 * Today, Forward, Ctrl+←/→, Ctrl+T; TODO B15 step 3), unless the view's own
 * widget says it.
 *
 * The widget says it when the focus is in it and the period moved: its
 * active cell or entry changes its name, and NVDA reads the name of what has
 * the focus — the date of the new cell, with the week where the week view's
 * cell carries it. Everywhere else the step used to be silent: the focus on a
 * toolbar button, on the check-in button, or a widget with nothing to read.
 * Today pressed on today moves nothing and is said as well, so the request
 * is never met with silence.
 *
 * `spoken` is the period in words, as the view shows it now; `widget` the
 * element with the focus model (grid or list); `widgetSpeaks` whether it has
 * an active cell or entry to read after the step (an empty day has none).
 */
export function usePageTurnAnnouncement(
  spoken: string,
  widget: RefObject<HTMLElement>,
  widgetSpeaks: boolean,
): void {
  const { pageTurn, anchor } = useViewState();
  const announce = useAnnouncer();
  const last = useRef({ pageTurn, anchor: anchor.getTime() });

  useEffect(() => {
    const before = last.current;
    last.current = { pageTurn, anchor: anchor.getTime() };
    if (pageTurn === before.pageTurn) return;
    const moved = anchor.getTime() !== before.anchor;
    const inWidget = widget.current?.contains(document.activeElement) ?? false;
    if (inWidget && moved && widgetSpeaks) return;
    announce(spoken);
  });
}
