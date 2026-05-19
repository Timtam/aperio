import { useCallback, useId, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { addDays, isSameDay, startOfWeek } from 'date-fns';

import { useAutoFocus } from '../../hooks/useAutoFocus';
import { localDateKey } from '../../intl/dateKey';
import { useDateFormat } from '../../intl/dateFormat';
import { useDialogState } from '../../state/DialogState';
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
 *  - `role="grid"` on the container, which holds focus permanently and
 *    announces the active cell via `aria-activedescendant`.
 *  - `role="gridcell"` per day, with `aria-selected` on the active cell.
 *  - `aria-current="date"` on today's cell.
 *
 * Why `aria-activedescendant` instead of a roving tabindex: with roving
 * tabindex the focused cell's `tabIndex` toggles between `0` and `-1`,
 * and the DOM focus has to be moved explicitly on each arrow press. If
 * the focus isn't moved the cell shows as selected (ARIA) but loses the
 * visual focus ring. The active-descendant pattern keeps DOM focus on
 * the grid; the cell highlight is pure CSS driven by a class, so there
 * is no window where "selected" and "highlighted" can disagree.
 *
 * Keyboard model:
 *  - Left/Right move the focused day inside the visible week.
 *  - Up/Down (and PageUp/PageDown) scroll the week — Outlook convention.
 *  - Home / End jump to the first / last day of the visible week.
 *  - Ctrl-modified arrows are handled by the global shortcut layer.
 *
 * `anchor` is the single source of truth; the focused cell index is
 * derived from it. One state update per key press, one render commit.
 */
export function WeekView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { anchor, setAnchor, goPrev, goNext } = useViewState();
  const { openEventDialog } = useDialogState();

  const range = useMemo(() => visibleRange('week', anchor), [anchor]);
  const { events, calendarById } = useEvents(range);

  const weekStart = useMemo(
    () => startOfWeek(anchor, { weekStartsOn: 1 }),
    [anchor],
  );
  const days = useMemo(
    () => Array.from({ length: 7 }, (_, i) => addDays(weekStart, i)),
    [weekStart],
  );

  const eventsByDay = useMemo(
    () => groupEventsByDay(events, days),
    [events, days],
  );

  const focusIndex = useMemo(() => {
    const i = days.findIndex((d) => isSameDay(d, anchor));
    return i >= 0 ? i : 0;
  }, [days, anchor]);

  // Unique prefix per WeekView instance, in case there's ever more than
  // one on screen (e.g. a future side-by-side comparison).
  const idPrefix = useId();
  const cellId = (i: number) => `${idPrefix}-cell-${i}`;

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      }
      switch (e.key) {
        case 'ArrowLeft':
          e.preventDefault();
          setAnchor(addDays(anchor, -1));
          return;
        case 'ArrowRight':
          e.preventDefault();
          setAnchor(addDays(anchor, 1));
          return;
        case 'ArrowUp':
        case 'PageUp':
          e.preventDefault();
          goPrev();
          return;
        case 'ArrowDown':
        case 'PageDown':
          e.preventDefault();
          goNext();
          return;
        case 'Home':
          e.preventDefault();
          setAnchor(weekStart);
          return;
        case 'End':
          e.preventDefault();
          setAnchor(addDays(weekStart, 6));
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          // Enter on a focused cell with events: open the first one.
          // Pressing Enter on an empty cell opens the quick-create
          // event dialog pre-seeded with that day.
          e.preventDefault();
          const focusedDay = days[focusIndex];
          const evs = eventsByDay.get(keyOf(focusedDay)) ?? [];
          if (evs.length > 0) {
            openEventDialog(evs[0]);
          } else {
            openEventDialog(null);
          }
          return;
        }
        default:
          return;
      }
    },
    [
      anchor,
      weekStart,
      setAnchor,
      goPrev,
      goNext,
      days,
      focusIndex,
      eventsByDay,
      openEventDialog,
    ],
  );

  const today = useMemo(() => new Date(), []);
  const isoWeek = fmt.isoWeek(weekStart);
  const gridRef = useAutoFocus<HTMLDivElement>();

  return (
    <section className="view view--week" aria-label={t('views.week.title')}>
      <header className="view__header">
        <h2>
          {t('views.week.kw', { week: isoWeek })} ·{' '}
          {fmt.format(weekStart, 'PP')} – {fmt.format(days[6], 'PP')}
        </h2>
      </header>

      <div
        ref={gridRef}
        role="grid"
        aria-label={t('views.week.gridLabel')}
        tabIndex={0}
        aria-activedescendant={cellId(focusIndex)}
        onKeyDown={handleKeyDown}
        className="week-grid"
      >
        <div role="row" className="week-grid__head">
          {days.map((day) => (
            <div
              key={day.toISOString()}
              role="columnheader"
              className="week-grid__col-head"
              aria-current={isSameDay(day, today) ? 'date' : undefined}
            >
              <span className="week-grid__dow">{fmt.format(day, 'EEE')}</span>
              <span className="week-grid__date">{fmt.format(day, 'd')}</span>
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
                id={cellId(i)}
                role="gridcell"
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
                onClick={() => setAnchor(day)}
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
                        <span
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
                        </span>
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
  // Local YYYY-MM-DD — see `localDateKey` for why this can't be
  // toISOString().slice(0, 10).
  return localDateKey(d);
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
