import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import type { CalendarEvent } from '../../api/types';
import { useDateFormat } from '../../intl/dateFormat';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';

/**
 * Chronological event list grouped by day.
 *
 * Screen-reader model (DESIGN.md section 3.3, Agenda):
 *  - The outer container is `role="list"`.
 *  - Each day group is a nested `role="listitem"` carrying the day
 *    summary (`aria-label="Wednesday, 14 May 2025, 2 events"`).
 *  - The events themselves are nested `listitem`s with the full event
 *    label.
 */
export function AgendaView() {
  const { t } = useTranslation();
  const { anchor } = useViewState();
  const fmt = useDateFormat();
  const range = useMemo(() => visibleRange('agenda', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);

  const groups = useMemo(() => groupByDay(events), [events]);

  return (
    <section aria-label={t('views.agenda.title')} className="view view--agenda">
      <header className="view__header">
        <h2>{t('views.agenda.title')}</h2>
        <span className="view__subtitle">
          {fmt.format(range.start, 'PP')} – {fmt.format(range.end, 'PP')}
        </span>
      </header>

      {loading && <p>{t('views.loading')}</p>}

      {!loading && events.length === 0 && (
        <p>{t('views.agenda.empty')}</p>
      )}

      <ul role="list" className="agenda-groups">
        {groups.map((group) => {
          const dayLabel = fmt.format(group.day, 'PPPP');
          return (
            <li
              key={group.day.toISOString()}
              role="listitem"
              aria-label={t('views.agenda.dayLabel', {
                day: dayLabel,
                count: group.events.length,
              })}
              className="agenda-group"
            >
              <h3 className="agenda-group__title">{dayLabel}</h3>
              <ul role="list" className="agenda-events">
                {group.events.map((ev) => {
                  const cal = calendarById.get(ev.calendar_id);
                  const timeLabel = ev.all_day
                    ? t('views.agenda.allDay')
                    : `${fmt.format(new Date(ev.start), 'p')} – ${fmt.format(
                        new Date(ev.end),
                        'p',
                      )}`;
                  const aria = t('views.agenda.eventLabel', {
                    title: ev.title,
                    time: timeLabel,
                    calendar: cal?.name ?? '—',
                  });
                  return (
                    <li
                      key={ev.id}
                      role="listitem"
                      aria-label={aria}
                      className="agenda-event"
                      style={
                        cal?.color
                          ? ({ '--event-color': cal.color.hex } as React.CSSProperties)
                          : undefined
                      }
                    >
                      <span className="agenda-event__time">{timeLabel}</span>
                      <span className="agenda-event__title">{ev.title}</span>
                      {cal && (
                        <span className="agenda-event__cal">{cal.name}</span>
                      )}
                    </li>
                  );
                })}
              </ul>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

interface DayGroup {
  day: Date;
  events: CalendarEvent[];
}

function groupByDay(events: CalendarEvent[]): DayGroup[] {
  const groups: DayGroup[] = [];
  events.forEach((ev) => {
    const day = new Date(ev.start);
    day.setHours(0, 0, 0, 0);
    const last = groups[groups.length - 1];
    if (last && isSameDay(last.day, day)) {
      last.events.push(ev);
    } else {
      groups.push({ day, events: [ev] });
    }
  });
  return groups;
}
