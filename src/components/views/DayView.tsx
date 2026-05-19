import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDateFormat } from '../../intl/dateFormat';
import { useDialogState } from '../../state/DialogState';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';

/**
 * Day view — flat listbox of the focused day's events.
 *
 * Phase 3 keeps it deliberately simple: the events of the active day are
 * rendered as `role="option"` items inside a `role="listbox"`, with
 * keyboard navigation via `aria-activedescendant`. The 15-minute slot
 * grid + slot focus from DESIGN.md section 3.3 returns alongside the
 * event-creation dialog in a later phase.
 *
 * Why listbox rather than a non-interactive list: with the listbox
 * pattern the screen reader stays in focus mode and reads the active
 * option as it changes — just like Week/Month view. A bare
 * `tabIndex=-1` section lacks a clear interactive role and lets NVDA
 * fall back to browse mode, which would break arrow navigation.
 */
export function DayView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { anchor } = useViewState();
  const { openEventDialog } = useDialogState();

  const range = useMemo(() => visibleRange('day', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);

  const dayEvents = useMemo(
    () => events.filter((ev) => isSameDay(new Date(ev.start), anchor)),
    [events, anchor],
  );

  const [focusIndex, setFocusIndex] = useState(0);

  // If the day changes (or events arrive) and the previous focus index
  // is past the end of the new list, snap back to the last valid item.
  useEffect(() => {
    if (focusIndex >= dayEvents.length) {
      setFocusIndex(Math.max(0, dayEvents.length - 1));
    }
  }, [dayEvents.length, focusIndex]);

  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-item-${i}`;
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (dayEvents.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, dayEvents.length - 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          setFocusIndex((i) => Math.max(i - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          setFocusIndex(0);
          return;
        case 'End':
          e.preventDefault();
          setFocusIndex(dayEvents.length - 1);
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          e.preventDefault();
          const ev = dayEvents[focusIndex];
          if (ev) openEventDialog(ev);
          return;
        }
        default:
          return;
      }
    },
    [dayEvents, focusIndex, openEventDialog],
  );

  const today = useMemo(() => new Date(), []);
  const isToday = isSameDay(today, anchor);

  return (
    <section className="view view--day" aria-label={fmt.format(anchor, 'PPPP')}>
      <header className="view__header">
        <h2 aria-current={isToday ? 'date' : undefined}>
          {fmt.format(anchor, 'PPPP')}
        </h2>
      </header>

      <ul
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.day.eventList')}
        aria-activedescendant={
          dayEvents.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="day-list"
      >
        {dayEvents.length === 0 ? (
          <li role="presentation" className="day-list__empty">
            {t('views.day.empty')}
          </li>
        ) : (
          dayEvents.map((ev, i) => {
            const cal = calendarById.get(ev.calendar_id);
            const startStr = fmt.format(new Date(ev.start), 'p');
            const endStr = fmt.format(new Date(ev.end), 'p');
            const aria = t('views.day.eventLabel', {
              title: ev.title,
              start: startStr,
              end: endStr,
              calendar: cal?.name ?? '—',
            });
            const focused = i === focusIndex;
            return (
              <li
                key={ev.id}
                id={itemId(i)}
                role="option"
                aria-selected={focused}
                aria-label={aria}
                className={
                  'day-list__item' +
                  (focused ? ' day-list__item--focused' : '')
                }
                style={
                  cal?.color
                    ? ({ '--event-color': cal.color.hex } as React.CSSProperties)
                    : undefined
                }
                onClick={() => setFocusIndex(i)}
              >
                <span className="day-list__time">
                  {startStr} – {endStr}
                </span>
                <span className="day-list__title">{ev.title}</span>
              </li>
            );
          })
        )}
      </ul>
    </section>
  );
}
