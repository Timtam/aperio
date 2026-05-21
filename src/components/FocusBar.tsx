import { useCallback, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import { useCalendarStore } from '../state/CalendarStore';
import { useDialogState } from '../state/DialogState';
import { useViewState } from '../state/ViewState';

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
      exitFocus();
      announce(t('sidebar.focus.exitedAnnouncement'));
    },
    [dialogMode, exitFocus, announce, t],
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
      exitFocus();
      announce(t('sidebar.focus.exitedAnnouncement'));
    }
  }, [focusedCalendarId, calendars, exitFocus, announce, t]);

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
        onClick={() => {
          exitFocus();
          announce(t('sidebar.focus.exitedAnnouncement'));
        }}
      >
        {t('sidebar.focus.exit')}
      </button>
    </div>
  );
}
