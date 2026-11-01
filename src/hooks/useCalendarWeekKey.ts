import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { calendarWeekOf, calendarWeekSentence } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { localDateKey } from '../intl/dateKey';
import type { ViewId } from '../state/viewMath';
import { useViewState } from '../state/viewStateContext';

/** The views that stand on a day, and so on a week. */
const CALENDAR_VIEWS: ReadonlySet<ViewId> = new Set([
  'day',
  'week',
  'month',
  'year',
  'agenda',
]);

/**
 * The K key: say the calendar week of the day the view stands on, with its
 * days — "Kalenderwoche 45, 2. bis 8. November 2026" (TODO B15). A request,
 * so it answers whether or not weeks are shown and announced otherwise.
 *
 * Only inside the calendar view itself. The sidebar finds entries by their
 * first letters, so a K there belongs to it, as it does in every text field;
 * the backlog is a region of its own that stands on no day, and its groups
 * name their weeks themselves.
 */
export function useCalendarWeekKey(): void {
  const { t, i18n } = useTranslation();
  const announce = useAnnouncer();
  const { view, anchor, weekStartsOn } = useViewState();

  useEffect(() => {
    if (!CALENDAR_VIEWS.has(view)) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key.toLowerCase() !== 'k') return;
      if (!(e.target instanceof HTMLElement)) return;
      const tag = e.target.tagName.toLowerCase();
      if (tag === 'input' || tag === 'textarea' || tag === 'select') return;
      if (e.target.isContentEditable) return;
      const region = e.target.closest<HTMLElement>('[data-region]');
      if (region?.dataset.region !== 'view') return;
      e.preventDefault();
      const week = calendarWeekOf(localDateKey(anchor), weekStartsOn);
      announce(calendarWeekSentence(t, i18n.language, week));
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [view, anchor, weekStartsOn, announce, t, i18n.language]);
}
