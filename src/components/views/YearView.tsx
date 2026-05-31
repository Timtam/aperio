import { useCallback, useId, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  addDays,
  addMonths,
  addYears,
  endOfMonth,
  endOfWeek,
  isSameDay,
  isSameMonth,
  setMonth,
  startOfMonth,
  startOfWeek,
} from 'date-fns';

import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDateFormat } from '../../intl/dateFormat';
import { useViewState } from '../../state/viewStateContext';

/**
 * Year view — 12 mini-month grids as a listbox.
 *
 * Keyboard model from DESIGN.md section 3.3:
 *  - Left/Right move to the previous/next month, wrapping across years.
 *  - Up/Down step a whole year while keeping the same month focused.
 *  - Enter opens the focused month in MonthView.
 *
 * The active month is derived from `anchor.getMonth()`. Changing the
 * focused month means updating `anchor`, which keeps the view state
 * coherent with Ctrl+T (today) and Ctrl+Left/Right (period steps from
 * the global shortcut layer).
 */
export function YearView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { anchor, setAnchor, setView } = useViewState();

  const months = useMemo(
    () => Array.from({ length: 12 }, (_, i) => setMonth(anchor, i)),
    [anchor],
  );
  const today = useMemo(() => new Date(), []);

  const focusIndex = anchor.getMonth();
  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-month-${i}`;
  const listRef = useAutoFocus<HTMLDivElement>();

  const openMonth = useCallback(
    (date: Date) => {
      setAnchor(date);
      setView('month');
    },
    [setAnchor, setView],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      switch (e.key) {
        case 'ArrowLeft':
          e.preventDefault();
          setAnchor(addMonths(anchor, -1));
          return;
        case 'ArrowRight':
          e.preventDefault();
          setAnchor(addMonths(anchor, 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          setAnchor(addYears(anchor, -1));
          return;
        case 'ArrowDown':
          e.preventDefault();
          setAnchor(addYears(anchor, 1));
          return;
        case 'Home':
          e.preventDefault();
          setAnchor(setMonth(anchor, 0));
          return;
        case 'End':
          e.preventDefault();
          setAnchor(setMonth(anchor, 11));
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar':
          e.preventDefault();
          openMonth(months[focusIndex]);
          return;
        default:
          return;
      }
    },
    [anchor, setAnchor, openMonth, months, focusIndex],
  );

  return (
    <section className="view view--year" aria-label={t('views.year.title')}>
      <header className="view__header">
        <h2>{fmt.format(anchor, 'yyyy')}</h2>
      </header>

      <div
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.year.gridLabel')}
        aria-activedescendant={itemId(focusIndex)}
        onKeyDown={handleKeyDown}
        className="year-grid"
      >
        {months.map((month, i) => {
          const cells = buildMiniGrid(month);
          const isCurrentMonth = isSameMonth(month, today);
          const focused = i === focusIndex;
          return (
            <div
              key={month.toISOString()}
              id={itemId(i)}
              role="option"
              aria-selected={focused}
              aria-current={isCurrentMonth ? 'date' : undefined}
              aria-label={t('views.year.monthLabel', {
                month: fmt.format(month, 'MMMM yyyy'),
              })}
              className={
                'year-mini' +
                (isCurrentMonth ? ' year-mini--current' : '') +
                (focused ? ' year-mini--focused' : '')
              }
              onClick={() => openMonth(month)}
              onDoubleClick={() => openMonth(month)}
            >
              <span className="year-mini__title">
                {fmt.format(month, 'MMMM')}
              </span>
              <div className="year-mini__grid" aria-hidden="true">
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
                      (!isSameMonth(day, month)
                        ? ' year-mini__day--outside'
                        : '') +
                      (isSameDay(day, today) ? ' year-mini__day--today' : '')
                    }
                  >
                    {fmt.format(day, 'd')}
                  </span>
                ))}
              </div>
            </div>
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
