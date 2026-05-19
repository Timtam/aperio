import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  addDays,
  endOfMonth,
  endOfWeek,
  isSameDay,
  isSameMonth,
  setMonth,
  startOfMonth,
  startOfWeek,
} from 'date-fns';

import { useDateFormat } from '../../intl/dateFormat';
import { useViewState } from '../../state/ViewState';

/**
 * Year view — 12 mini-month grids.
 *
 * Phase 3 keeps it compact: each month is a small read-only grid. Arrow
 * keys move between months (one per column, three per row); Enter jumps
 * to MonthView for the focused month.
 *
 * Event counts per day are intentionally *not* shown here. The year view
 * is a planning lens — pulling events for 12 months would be heavy, and
 * the screen real estate doesn't support showing them anyway. A future
 * phase may overlay heat-map dots for "days with events" once we add a
 * lightweight backend count endpoint.
 */
export function YearView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { anchor, setView, setAnchor } = useViewState();

  const months = useMemo(
    () => Array.from({ length: 12 }, (_, i) => setMonth(anchor, i)),
    [anchor],
  );
  const today = useMemo(() => new Date(), []);

  return (
    <section className="view view--year" aria-label={t('views.year.title')}>
      <header className="view__header">
        <h2>{fmt.format(anchor, 'yyyy')}</h2>
      </header>

      <div className="year-grid" role="list" aria-label={t('views.year.gridLabel')}>
        {months.map((month) => {
          const cells = buildMiniGrid(month);
          const isCurrentMonth = isSameMonth(month, today);
          return (
            <button
              key={month.toISOString()}
              type="button"
              role="listitem"
              aria-label={t('views.year.monthLabel', {
                month: fmt.format(month, 'MMMM yyyy'),
              })}
              aria-current={isCurrentMonth ? 'date' : undefined}
              className={
                'year-mini' + (isCurrentMonth ? ' year-mini--current' : '')
              }
              onClick={() => {
                setAnchor(month);
                setView('month');
              }}
            >
              <span className="year-mini__title">{fmt.format(month, 'MMMM')}</span>
              <div className="year-mini__grid">
                {cells.slice(0, 7).map((d) => (
                  <span key={d.toISOString()} className="year-mini__dow">
                    {fmt.format(d, 'EEEEE')}
                  </span>
                ))}
                {cells.map((day) => (
                  <span
                    key={day.toISOString()}
                    className={
                      'year-mini__day' +
                      (!isSameMonth(day, month) ? ' year-mini__day--outside' : '') +
                      (isSameDay(day, today) ? ' year-mini__day--today' : '')
                    }
                    aria-hidden="true"
                  >
                    {fmt.format(day, 'd')}
                  </span>
                ))}
              </div>
            </button>
          );
        })}
      </div>
    </section>
  );
}

function buildMiniGrid(month: Date): Date[] {
  const first = startOfMonth(month);
  const last = endOfMonth(month);
  const start = startOfWeek(first, { weekStartsOn: 1 });
  const end = endOfWeek(last, { weekStartsOn: 1 });
  const out: Date[] = [];
  let cur = start;
  while (cur <= end) {
    out.push(cur);
    cur = addDays(cur, 1);
  }
  return out;
}
