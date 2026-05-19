import { useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  addDays,
  endOfMonth,
  endOfWeek,
  isSameDay,
  isSameMonth,
  startOfMonth,
  startOfWeek,
} from 'date-fns';

import { useAnnouncer } from '../../a11y/Announcer';
import { useDateFormat } from '../../intl/dateFormat';
import { useGridNavigation } from '../../hooks/useGridNavigation';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';
import type { CalendarEvent } from '../../api/types';

/**
 * Month view — six-week calendar grid (DESIGN.md section 3.3,
 * Monatsansicht).
 *
 * The visible range covers the calendar month, but the grid renders the
 * full weeks that touch it (typically 35 or 42 cells). Cells outside the
 * anchor month are dimmed but still focusable.
 *
 * Keyboard model is identical to the week view: Left/Right between days,
 * Up/Down between weeks (within the visible block), Home/End to row
 * ends. An optional KW column is rendered on the left.
 */
export function MonthView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor, setAnchor } = useViewState();

  // Range fed into useEvents — covers the actual visible cells, not just
  // the calendar month, so events bleeding in from neighbouring months
  // still show up.
  const cells = useMemo(() => buildMonthGrid(anchor), [anchor]);
  const range = useMemo(() => visibleRange('month', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);

  const eventsByDay = useMemo(() => groupEventsByDay(events), [events]);

  const initialIndex = useMemo(() => {
    const i = cells.findIndex((c) => isSameDay(c, anchor));
    return i >= 0 ? i : 0;
  }, [cells, anchor]);

  const { focusIndex, setFocusIndex, handleKeyDown } = useGridNavigation({
    itemCount: cells.length,
    rowSize: 7,
    initialIndex,
  });

  useEffect(() => {
    setAnchor(cells[focusIndex]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusIndex]);

  const lastAnnouncedRef = useRef<string>('');
  useEffect(() => {
    const day = cells[focusIndex];
    if (!day) return;
    const evs = eventsByDay.get(keyOf(day)) ?? [];
    const label = t('views.month.dayAnnounce', {
      day: fmt.format(day, 'PPPP'),
      count: evs.length,
    });
    if (label !== lastAnnouncedRef.current) {
      lastAnnouncedRef.current = label;
      announce(label);
    }
  }, [focusIndex, cells, eventsByDay, announce, fmt, t]);

  const today = useMemo(() => new Date(), []);
  const rowCount = cells.length / 7;

  return (
    <section className="view view--month" aria-label={t('views.month.title')}>
      <header className="view__header">
        <h2>{fmt.format(anchor, 'MMMM yyyy')}</h2>
      </header>

      {loading && <p>{t('views.loading')}</p>}

      <div
        role="grid"
        aria-label={t('views.month.gridLabel')}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        className="month-grid"
      >
        <div role="row" className="month-grid__head">
          <div role="columnheader" className="month-grid__kw" aria-label="KW">
            {t('views.month.kwShort')}
          </div>
          {cells.slice(0, 7).map((d) => (
            <div
              key={d.toISOString()}
              role="columnheader"
              className="month-grid__col-head"
            >
              {fmt.format(d, 'EEE')}
            </div>
          ))}
        </div>

        {Array.from({ length: rowCount }, (_, row) => {
          const rowStart = cells[row * 7];
          return (
            <div role="row" key={rowStart.toISOString()} className="month-grid__row">
              <div role="rowheader" className="month-grid__kw">
                {fmt.isoWeek(rowStart)}
              </div>
              {cells.slice(row * 7, row * 7 + 7).map((day, col) => {
                const flatIndex = row * 7 + col;
                const dayEvents = eventsByDay.get(keyOf(day)) ?? [];
                const focused = flatIndex === focusIndex;
                const outside = !isSameMonth(day, anchor);
                return (
                  <div
                    key={day.toISOString()}
                    role="gridcell"
                    tabIndex={focused ? 0 : -1}
                    aria-selected={focused}
                    aria-current={isSameDay(day, today) ? 'date' : undefined}
                    aria-label={t('views.month.dayAnnounce', {
                      day: fmt.format(day, 'PPPP'),
                      count: dayEvents.length,
                    })}
                    className={
                      'month-grid__cell' +
                      (focused ? ' month-grid__cell--focused' : '') +
                      (outside ? ' month-grid__cell--outside' : '') +
                      (isSameDay(day, today) ? ' month-grid__cell--today' : '')
                    }
                    onClick={() => setFocusIndex(flatIndex)}
                  >
                    <span className="month-grid__date">
                      {fmt.format(day, 'd')}
                    </span>
                    {dayEvents.slice(0, 3).map((ev) => {
                      const cal = calendarById.get(ev.calendar_id);
                      return (
                        <span
                          key={ev.id}
                          className="month-event"
                          style={
                            cal?.color
                              ? ({ '--event-color': cal.color.hex } as React.CSSProperties)
                              : undefined
                          }
                        >
                          {ev.title}
                        </span>
                      );
                    })}
                    {dayEvents.length > 3 && (
                      <span className="month-event month-event--more">
                        {t('views.month.moreEvents', {
                          count: dayEvents.length - 3,
                        })}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </section>
  );
}

/**
 * Build the 35- or 42-cell grid for the month containing `anchor`.
 * Always starts at Monday of the week containing the first of the month
 * and ends at Sunday of the week containing the last of the month.
 */
function buildMonthGrid(anchor: Date): Date[] {
  const first = startOfMonth(anchor);
  const last = endOfMonth(anchor);
  const gridStart = startOfWeek(first, { weekStartsOn: 1 });
  const gridEnd = endOfWeek(last, { weekStartsOn: 1 });
  const out: Date[] = [];
  let cur = gridStart;
  while (cur <= gridEnd) {
    out.push(cur);
    cur = addDays(cur, 1);
  }
  return out;
}

function keyOf(d: Date): string {
  return d.toISOString().slice(0, 10);
}

function groupEventsByDay(events: CalendarEvent[]): Map<string, CalendarEvent[]> {
  const map = new Map<string, CalendarEvent[]>();
  events.forEach((ev) => {
    const k = keyOf(new Date(ev.start));
    const bucket = map.get(k);
    if (bucket) bucket.push(ev);
    else map.set(k, [ev]);
  });
  return map;
}
