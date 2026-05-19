import { useEffect, useRef, type RefObject } from 'react';

/**
 * Focus the referenced element on mount.
 *
 * Used by every view component so switching to a new view (Ctrl+1..6, a
 * toolbar click, or the initial load) lands keyboard focus inside that
 * view's primary surface — the grid, list, or section — instead of
 * leaving it on `<body>` where arrow keys would do nothing.
 *
 * `preventScroll: true` keeps the page from jumping when the element
 * happens to be off-screen; the view's own scroll handling decides
 * where to put it.
 *
 * The browser's `:focus-visible` heuristic still applies: a programmatic
 * focus following a keyboard event (Ctrl+1) shows the focus ring; one
 * following a mouse click does not. That's the right behaviour for
 * both input modalities.
 */
export function useAutoFocus<T extends HTMLElement>(): RefObject<T> {
  const ref = useRef<T>(null);
  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
  }, []);
  return ref;
}
