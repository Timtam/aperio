import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import { useDateFormat } from '../../intl/dateFormat';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';

const HOUR_START = 7;
const HOUR_END = 22;

/**
 * Day view — single-day time grid (DESIGN.md section 3.3, "Tagesansicht").
 *
 * Phase 3 ships the structural shell: a vertical list of one-hour rows,
 * with events anchored to the row that contains their start time. The
 * fine-grained 15-minute slot navigation, keyboard slot focus, and
 * resize-on-drag come with the keyboard-shortcut and dialog wave
 * (Phases 4–5).
 *
 * ARIA model: `role="listbox"` for the column, `role="option"` for each
 * event so screen readers can step through them with Tab.
 */
export function DayView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { anchor } = useViewState();

  const range = useMemo(() => visibleRange('day', anchor), [anchor]);
  const { events, calendarById } = useEvents(range);

  const dayEvents = useMemo(
    () => events.filter((ev) => isSameDay(new Date(ev.start), anchor)),
    [events, anchor],
  );

  const hours = useMemo(() => {
    const out: number[] = [];
    for (let h = HOUR_START; h <= HOUR_END; h++) out.push(h);
    return out;
  }, []);

  const eventsByHour = useMemo(() => {
    const map = new Map<number, typeof dayEvents>();
    dayEvents.forEach((ev) => {
      const startHour = new Date(ev.start).getHours();
      const bucket = clamp(startHour, HOUR_START, HOUR_END);
      const existing = map.get(bucket) ?? [];
      existing.push(ev);
      map.set(bucket, existing);
    });
    return map;
  }, [dayEvents]);

  const today = useMemo(() => new Date(), []);
  const isToday = isSameDay(today, anchor);

  return (
    <section
      aria-label={fmt.format(anchor, 'PPPP')}
      className="view view--day"
    >
      <header className="view__header">
        <h2 aria-current={isToday ? 'date' : undefined}>
          {fmt.format(anchor, 'PPPP')}
        </h2>
      </header>

      <ul role="listbox" aria-label={t('views.day.timeGrid')} className="day-grid">
        {hours.map((hour) => {
          const slotEvents = eventsByHour.get(hour) ?? [];
          return (
            <li
              key={hour}
              role="presentation"
              className="day-grid__row"
              data-hour={hour}
            >
              <span className="day-grid__hour" aria-hidden="true">
                {fmt.format(setHour(anchor, hour), 'p')}
              </span>
              <div className="day-grid__slots">
                {slotEvents.map((ev) => {
                  const cal = calendarById.get(ev.calendar_id);
                  const startStr = fmt.format(new Date(ev.start), 'p');
                  const endStr = fmt.format(new Date(ev.end), 'p');
                  const aria = t('views.day.eventLabel', {
                    title: ev.title,
                    start: startStr,
                    end: endStr,
                    calendar: cal?.name ?? '—',
                  });
                  return (
                    <div
                      key={ev.id}
                      role="option"
                      aria-selected="false"
                      aria-label={aria}
                      tabIndex={0}
                      className="day-event"
                      style={
                        cal?.color
                          ? ({ '--event-color': cal.color.hex } as React.CSSProperties)
                          : undefined
                      }
                    >
                      <span className="day-event__time">
                        {startStr}–{endStr}
                      </span>
                      <span className="day-event__title">{ev.title}</span>
                    </div>
                  );
                })}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function setHour(base: Date, hour: number): Date {
  const d = new Date(base);
  d.setHours(hour, 0, 0, 0);
  return d;
}

function clamp(value: number, min: number, max: number): number {
  if (value < min) return min;
  if (value > max) return max;
  return value;
}
