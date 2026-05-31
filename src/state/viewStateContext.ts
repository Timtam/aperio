import { createContext, useContext, useEffect } from 'react';

import { VIEWS, type ViewId } from './viewMath';

/**
 * Active-view state context + consumer hooks. Split out of
 * `ViewStateProvider` so that component file exports only its
 * component (Fast Refresh). The provider implementation lives there.
 */
export interface ViewStateValue {
  view: ViewId;
  anchor: Date;
  setView: (v: ViewId) => void;
  setAnchor: (d: Date) => void;
  jumpToToday: () => void;
  goPrev: () => void;
  goNext: () => void;
  /**
   * Active "drill-in" calendar. When set, every view reads only this
   * calendar's events; the sidebar's own checkbox selection is
   * preserved verbatim and restored when focus exits. `null` is the
   * default "show all the sidebar's selected calendars" mode.
   */
  focusedCalendarId: string | null;
  enterFocus: (calendarId: string) => void;
  exitFocus: () => void;
}

export const ViewStateContext = createContext<ViewStateValue | null>(null);

export function useViewState(): ViewStateValue {
  const ctx = useContext(ViewStateContext);
  if (!ctx) {
    throw new Error('useViewState must be used inside <ViewStateProvider>');
  }
  return ctx;
}

/**
 * Wire global keyboard shortcuts to the view state.
 *
 * - Ctrl/Cmd + 1..6 → switch view in the order from `viewMath.VIEWS`.
 * - Ctrl/Cmd + T   → jump to today.
 * - Ctrl/Cmd + ←/→ → previous/next period.
 *
 * Ignores keystrokes while the user is typing in a form control.
 */
export function useViewShortcuts(): void {
  const { setView, jumpToToday, goPrev, goNext } = useViewState();

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (isEditableTarget(e.target)) return;
      const cmd = e.ctrlKey || e.metaKey;
      if (!cmd) return;

      if (e.key >= '1' && e.key <= '9') {
        const index = Number(e.key) - 1;
        const next = VIEWS[index];
        if (next) {
          e.preventDefault();
          setView(next);
        }
        return;
      }

      const k = e.key.toLowerCase();
      if (k === 't') {
        e.preventDefault();
        jumpToToday();
        return;
      }
      if (e.key === 'ArrowLeft') {
        e.preventDefault();
        goPrev();
        return;
      }
      if (e.key === 'ArrowRight') {
        e.preventDefault();
        goNext();
        return;
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [setView, jumpToToday, goPrev, goNext]);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return true;
  if (target.isContentEditable) return true;
  return false;
}
