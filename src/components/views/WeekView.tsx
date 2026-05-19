import { useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { addDays, isSameDay, startOfWeek } from 'date-fns';

import { useAnnouncer } from '../../a11y/Announcer';
import { useDateFormat } from '../../intl/dateFormat';
import { useGridNavigation } from '../../hooks/useGridNavigation';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';
import type { CalendarEvent } from '../../api/types';

/**
 * Week view — the workhorse calendar surface.
 *
 * Layout: a 7-cell grid (Mon–Sun, ISO weeks). Each cell shows the events
 * scheduled on that day. The KW number lives in the header next to the
 * date range (DESIGN.md section 5.2).
 *
 * Screen-reader model (section 3.3, Wochenansicht):
 *  - `role="grid"` on the container.
 *  - `role="gridcell"` per day, `aria-selected` flipped on the focused cell.
 *  - Polite announcement when the focused day changes: full date plus
 *    event count.
 *  - `aria-current="date"` on today's cell.
 *
 * Keyboard: handled by `useGridNavigation` — Left/Right between days,
 * Up/Down between weeks (shifts the anchor and re-fetches). Tab traversal
 * between events inside a day is the browser's default behaviour because
 * each event renders as a focusable button.
 */
export function WeekView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor, setAnchor } = useViewState();

  const range = useMemo(() => visibleRange('week', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);

  const weekStart = useMemo(
    () => startOfWeek(anchor, { weekStartsOn: 1 }),
    [anchor],
  );
  const days = useMemo(
    () => Array.from({ length: 7 }, (_, i) => addDays(weekStart, i)),
    [weekStart],
  );

  const eventsByDay = useMemo(() => groupEventsByDay(events, days), [events, days]);

  // Focus index: which of the seven cells is currently focused. Persist
  // across week changes by mapping today's weekday onto the new anchor.
  const initialIndex = useMemo(() => {
    const idx = days.findIndex((d) => isSameDay(d, anchor));
    return idx >= 0 ? idx : 0;
  }, [days, anchor]);

  const { focusIndex, setFocusIndex, handleKeyDown } = useGridNavigation({
    itemCount: 7,
    rowSize: 7,
    initialIndex,
  });

  // Sync the anchor when the focused day changes. The hook clamps inside
  // [0,6]; arrow Up/Down outside that range falls through to the caller,
  // which we use to shift weeks.
  useEffect(() => {
    setAnchor(days[focusIndex]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusIndex]);

  // Announce the focused day when it changes.
  const lastAnnouncedRef = useRef<string>('');
  useEffect(() => {
    const day = days[focusIndex];
    if (!day) return;
    const evs = eventsByDay.get(keyOf(day)) ?? [];
    const label = t('views.week.dayAnnounce', {
      day: fmt.format(day, 'PPPP'),
      count: evs.length,
    });
    if (label !== lastAnnouncedRef.current) {
      lastAnnouncedRef.current = label;
      announce(label);
    }
  }, [focusIndex, days, eventsByDay, announce, fmt, t]);

  const today = useMemo(() => new Date(), []);
  const isoWeek = fmt.isoWeek(weekStart);

  return (
    <section className="view view--week" aria-label={t('views.week.title')}>
      <header className="view__header">
        <h2>
          {t('views.week.kw', { week: isoWeek })} ·{' '}
          {fmt.format(weekStart, 'PP')} – {fmt.format(days[6], 'PP')}
        </h2>
      </header>

      {loading && <p>{t('views.loading')}</p>}

      <div
        role="grid"
        aria-label={t('views.week.gridLabel')}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        className="week-grid"
      >
        <div role="row" className="week-grid__head">
          {days.map((day, i) => (
            <div
              key={day.toISOString()}
              role="columnheader"
              className="week-grid__col-head"
              aria-current={isSameDay(day, today) ? 'date' : undefined}
            >
              <span className="week-grid__dow">{fmt.format(day, 'EEE')}</span>
              <button
                type="button"
                className="week-grid__date"
                aria-label={fmt.format(day, 'PPPP')}
                onClick={() => setFocusIndex(i)}
              >
                {fmt.format(day, 'd')}
              </button>
            </div>
          ))}
        </div>

        <div role="row" className="week-grid__body">
          {days.map((day, i) => {
            const dayEvents = eventsByDay.get(keyOf(day)) ?? [];
            const focused = i === focusIndex;
            return (
              <div
                key={day.toISOString()}
                role="gridcell"
                tabIndex={focused ? 0 : -1}
                aria-selected={focused}
                aria-current={isSameDay(day, today) ? 'date' : undefined}
                aria-label={t('views.week.dayAnnounce', {
                  day: fmt.format(day, 'PPPP'),
                  count: dayEvents.length,
                })}
                className={
                  'week-grid__cell' +
                  (focused ? ' week-grid__cell--focused' : '') +
                  (isSameDay(day, today) ? ' week-grid__cell--today' : '')
                }
                onClick={() => setFocusIndex(i)}
              >
                <ul role="list" className="week-grid__events">
                  {dayEvents.map((ev) => {
                    const cal = calendarById.get(ev.calendar_id);
                    const time = ev.all_day
                      ? t('views.allDay')
                      : `${fmt.format(new Date(ev.start), 'p')}`;
                    const aria = t('views.week.eventLabel', {
                      title: ev.title,
                      time,
                      calendar: cal?.name ?? '—',
                    });
                    return (
                      <li key={ev.id} role="listitem">
                        <button
                          type="button"
                          className="week-event"
                          aria-label={aria}
                          style={
                            cal?.color
                              ? ({ '--event-color': cal.color.hex } as React.CSSProperties)
                              : undefined
                          }
                        >
                          <span className="week-event__time">{time}</span>
                          <span className="week-event__title">{ev.title}</span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function keyOf(d: Date): string {
  return d.toISOString().slice(0, 10);
}

function groupEventsByDay(
  events: CalendarEvent[],
  days: Date[],
): Map<string, CalendarEvent[]> {
  const map = new Map<string, CalendarEvent[]>();
  days.forEach((d) => map.set(keyOf(d), []));
  events.forEach((ev) => {
    const k = keyOf(new Date(ev.start));
    const bucket = map.get(k);
    if (bucket) bucket.push(ev);
  });
  return map;
}
