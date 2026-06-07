import {
  Fragment,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from 'react';
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

import { useAnnouncer } from '../../a11y/announcerContext';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useEventTabNavigation } from '../../hooks/useEventTabNavigation';
import { localDateKey } from '../../intl/dateKey';
import {
  isExpandedOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../../intl/recurrence';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveEventColor } from '../../intl/eventColor';
import {
  buildAllDayBars,
  daysCoveredKeys,
  multiDayInfo,
} from '../../intl/multiDay';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange } from '../../state/viewMath';
import type { CalendarEvent } from '../../api/types';
import { BacklogRail } from '../BacklogRail';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';
import {
  readTaskDrag,
  scheduleTaskOnDay,
  TASK_DND_TYPE,
} from '../../state/moveActions';

/**
 * Month view — six-week calendar grid (DESIGN.md section 3.3,
 * Monatsansicht).
 *
 * Uses the same `aria-activedescendant` focus model as WeekView:
 * DOM focus stays on the grid container, the active cell is referenced
 * by id, the highlight is a CSS class on that cell. See WeekView for
 * the rationale.
 *
 * Keyboard model: Left/Right shift the anchor by one day, Up/Down by
 * seven days, Home / End jump to the start / end of the current row, and
 * PageUp/PageDown step a whole month. Ctrl-modified arrows are handled
 * by the global shortcut layer.
 */
export function MonthView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor, setAnchor, goPrev, goNext } = useViewState();
  const { openEventDialog, invalidateData } = useDialogState();
  const { openForEvent: openEventMenu } = useChipContextMenu();

  const cells = useMemo(() => buildMonthGrid(anchor), [anchor]);
  const range = useMemo(() => visibleRange('month', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  const eventsByDay = useMemo(() => groupEventsByDay(events), [events]);

  const focusIndex = useMemo(() => {
    const i = cells.findIndex((c) => isSameDay(c, anchor));
    return i >= 0 ? i : 0;
  }, [cells, anchor]);

  const idPrefix = useId();
  const cellId = (i: number) => `${idPrefix}-cell-${i}`;
  const eventOptionId = useCallback(
    (cellIdx: number, evIdx: number) =>
      `${idPrefix}-cell-${cellIdx}-ev-${evIdx}`,
    [idPrefix],
  );

  // Two-level focus mirrors WeekView; the tab hook below handles
  // chronological cycling across cells.
  const buckets = useMemo(
    () => cells.map((d) => ({ items: eventsByDay.get(keyOf(d)) ?? [] })),
    [cells, eventsByDay],
  );
  const focusedDayEvents = useMemo(
    () => buckets[focusIndex]?.items ?? [],
    [buckets, focusIndex],
  );

  const dayChangeAnnouncer = useCallback(
    (newDayIdx: number, ev: CalendarEvent) => {
      announce(
        t('views.month.tabAnnounce', {
          day: fmt.format(cells[newDayIdx], 'PPPP'),
          title: ev.title,
        }),
      );
    },
    [announce, cells, fmt, t],
  );

  const {
    eventIndex,
    clear: clearEventIndex,
    handleTab,
  } = useEventTabNavigation({
    buckets,
    dayIndex: focusIndex,
    setDayIndex: (next) => setAnchor(cells[next]),
    onDayChange: dayChangeAnnouncer,
  });

  // Same lane model as WeekView, but applied per week-row of the
  // month — the bar can't run across a Sunday/Monday break, so we
  // call buildAllDayBars once per 7-day slice. See WeekView for the
  // SR/visual split.
  const focusedEvId =
    eventIndex !== null
      ? (buckets[focusIndex]?.items[eventIndex]?.id ?? null)
      : null;

  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const [scopeTarget, setScopeTarget] = useState<CalendarEvent | null>(null);

  const performDelete = useCallback(
    async (ev: CalendarEvent, scope: 'occurrence' | 'series') => {
      try {
        const occIso = occurrenceIsoOf(ev);
        if (scope === 'occurrence' && occIso) {
          await addEventExdate(seriesIdOf(ev), occIso, ev.calendar_id);
          announce(
            t('dialogs.event.occurrenceDeleted', { title: ev.title }),
          );
        } else {
          await deleteEventById(seriesIdOf(ev), ev.calendar_id);
          announce(t('dialogs.event.deleted', { title: ev.title }));
        }
        // Local view-state dialogs don't go through DialogState.close(),
        // so the dataVersion bump has to be explicit here.
        invalidateData();
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t, invalidateData],
  );

  const requestDelete = useCallback((ev: CalendarEvent) => {
    if (isExpandedOccurrence(ev) || ev.recurrence) {
      setScopeTarget(ev);
    } else {
      setConfirmTarget(ev);
    }
  }, []);

  // Drag-and-drop: a task dropped on a day cell is scheduled on that day
  // (e.g. dragged out of the backlog rail). Mouse affordance only.
  const scheduleByDrop = useCallback(
    async (day: Date, e: React.DragEvent) => {
      e.preventDefault();
      const payload = readTaskDrag(e.dataTransfer);
      if (!payload) return;
      const dayKey = localDateKey(day);
      if (payload.task.scheduled_date === dayKey) return;
      try {
        await scheduleTaskOnDay(payload.task, dayKey);
        invalidateData();
        announce(
          t('views.backlog.scheduled', {
            title: payload.task.title,
            date: fmt.format(day, 'PP'),
          }),
        );
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [invalidateData, announce, t, fmt],
  );

  // Deferred indicator — see DayView for the rationale.
  const showLoading = useDeferredLoading(loading);
  useEffect(() => {
    if (showLoading) announce(t('views.loading'));
  }, [showLoading, announce, t]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Tab' && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const consumed = handleTab(e.shiftKey);
        if (consumed) e.preventDefault();
        return;
      }
      if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      }
      if (eventIndex !== null) {
        const ev = focusedDayEvents[eventIndex];
        if (e.key === 'Escape') {
          e.preventDefault();
          clearEventIndex();
          return;
        }
        if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          if (ev) openEventDialog(ev);
          return;
        }
        if (e.key === 'Delete' || e.key === 'Backspace') {
          e.preventDefault();
          if (ev) requestDelete(ev);
          return;
        }
        if (
          e.key === 'ContextMenu' ||
          (e.shiftKey && e.key === 'F10')
        ) {
          e.preventDefault();
          if (ev) {
            const target = e.currentTarget as HTMLElement;
            const id = eventOptionId(focusIndex, eventIndex);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            const pos = rect
              ? { x: rect.left, y: rect.bottom }
              : undefined;
            void openEventMenu(ev, pos);
          }
          return;
        }
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
          e.preventDefault();
          setAnchor(addDays(anchor, -7));
          return;
        case 'ArrowDown':
          e.preventDefault();
          setAnchor(addDays(anchor, 7));
          return;
        case 'Home': {
          e.preventDefault();
          setAnchor(startOfWeek(anchor, { weekStartsOn: 1 }));
          return;
        }
        case 'End': {
          e.preventDefault();
          setAnchor(endOfWeek(anchor, { weekStartsOn: 1 }));
          return;
        }
        case 'PageUp':
          e.preventDefault();
          goPrev();
          return;
        case 'PageDown':
          e.preventDefault();
          goNext();
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          e.preventDefault();
          const focusedDay = cells[focusIndex];
          const evs = eventsByDay.get(keyOf(focusedDay)) ?? [];
          if (evs.length > 0) {
            openEventDialog(evs[0]);
          } else {
            openEventDialog(null, {
              defaultDate: keyOf(focusedDay),
            });
          }
          return;
        }
        default:
          return;
      }
    },
    [
      anchor,
      setAnchor,
      goPrev,
      goNext,
      cells,
      focusIndex,
      eventIndex,
      focusedDayEvents,
      eventsByDay,
      openEventDialog,
      handleTab,
      clearEventIndex,
      requestDelete,
      openEventMenu,
      eventOptionId,
    ],
  );

  const today = useMemo(() => new Date(), []);
  const rowCount = cells.length / 7;
  const gridRef = useAutoFocus<HTMLDivElement>(!loading);

  // How many event chips fit in a day cell depends on the cell height,
  // which depends on the window height (the grid fills the available
  // space). Measure a row after layout and recompute on resize so a taller
  // window shows more events instead of empty space — and a shorter one
  // collapses gracefully to a "+N" hint. Overflow events stay in the DOM
  // either way (see the render note); this only changes how many are
  // visible.
  const [visiblePerCell, setVisiblePerCell] = useState(3);
  useEffect(() => {
    const grid = gridRef.current;
    if (!grid || typeof ResizeObserver === 'undefined') return;
    const recompute = () => {
      const row = grid.querySelector('.month-grid__row');
      const rowH = row?.getBoundingClientRect().height ?? 0;
      if (rowH <= 0) return;
      // Rough cell budget for the xs font: date line + cell padding, then
      // one chip per event. Being off by a chip is harmless — the cell
      // clips overflow, and an extra event just becomes "+N more".
      const CELL_OVERHEAD = 28;
      const CHIP = 21;
      const fit = Math.max(1, Math.floor((rowH - CELL_OVERHEAD) / CHIP));
      setVisiblePerCell((prev) => (prev === fit ? prev : fit));
    };
    recompute();
    const ro = new ResizeObserver(recompute);
    ro.observe(grid);
    return () => ro.disconnect();
  }, [gridRef, rowCount]);

  return (
    <section className="view view--month" aria-label={t('views.month.title')}>
      <header className="view__header">
        <h2>{fmt.format(anchor, 'MMMM yyyy')}</h2>
      </header>

      {showLoading && (
        <p className="view__loading" aria-hidden="true">
          {t('views.loading')}
        </p>
      )}

      <div
        ref={gridRef}
        role="grid"
        aria-label={t('views.month.gridLabel')}
        tabIndex={0}
        aria-activedescendant={
          eventIndex !== null
            ? eventOptionId(focusIndex, eventIndex)
            : cellId(focusIndex)
        }
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
          const rowCells = cells.slice(row * 7, row * 7 + 7);
          const rowBars = buildAllDayBars(events, rowCells);
          const laneRows = rowBars.reduce(
            (m, b) => Math.max(m, b.lane + 1),
            0,
          );
          return (
            <Fragment key={rowStart.toISOString()}>
              {rowBars.length > 0 && (
                <div
                  className="month-grid__lane"
                  aria-hidden="true"
                  style={
                    { '--lane-rows': laneRows } as React.CSSProperties
                  }
                >
                  {rowBars.map((bar) => {
                    const color = resolveEventColor(
                      bar.event,
                      calendarById,
                      labelById,
                    );
                    const isBarFocused =
                      focusedEvId === bar.event.id;
                    // The lane shares the row's 8-column layout
                    // (40 px KW + 7 day columns), so day columns
                    // start at grid index 2.
                    const style: React.CSSProperties &
                      Record<string, string> = {
                      gridColumn: `${bar.startCol + 1} / ${bar.endCol + 2}`,
                      gridRow: String(bar.lane + 1),
                    };
                    if (color.hex)
                      style['--event-color'] = color.hex;
                    return (
                      <div
                        key={bar.event.id}
                        className={
                          'month-allday-bar' +
                          (isBarFocused
                            ? ' month-allday-bar--focused'
                            : '') +
                          (bar.continuesBefore
                            ? ' month-allday-bar--continues-before'
                            : '') +
                          (bar.continuesAfter
                            ? ' month-allday-bar--continues-after'
                            : '')
                        }
                        style={style}
                        onClick={() => openEventDialog(bar.event)}
                        title={bar.event.title}
                      >
                        {bar.continuesBefore && (
                          <span
                            className="month-allday-bar__chevron"
                            aria-hidden="true"
                          >
                            ‹
                          </span>
                        )}
                        <span className="month-allday-bar__title">
                          {bar.event.title}
                        </span>
                        {bar.continuesAfter && (
                          <span
                            className="month-allday-bar__chevron"
                            aria-hidden="true"
                          >
                            ›
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
              <div role="row" className="month-grid__row">
              <div role="rowheader" className="month-grid__kw">
                {fmt.isoWeek(rowStart)}
              </div>
              {cells.slice(row * 7, row * 7 + 7).map((day, col) => {
                const flatIndex = row * 7 + col;
                const dayEvents = eventsByDay.get(keyOf(day)) ?? [];
                const focused = flatIndex === focusIndex;
                const outside = !isSameMonth(day, anchor);
                // When there are more events than fit, reserve the last
                // visible slot for the "+N more" hint so the cell never
                // overflows its row.
                const visibleLimit =
                  dayEvents.length > visiblePerCell
                    ? Math.max(0, visiblePerCell - 1)
                    : visiblePerCell;
                const moreCount = dayEvents.length - visibleLimit;
                return (
                  <div
                    key={day.toISOString()}
                    id={cellId(flatIndex)}
                    role="gridcell"
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
                    onClick={() => setAnchor(day)}
                    onDragOver={(e) => {
                      if (!e.dataTransfer.types.includes(TASK_DND_TYPE)) return;
                      e.preventDefault();
                      e.dataTransfer.dropEffect = 'move';
                    }}
                    onDrop={(e) => void scheduleByDrop(day, e)}
                  >
                    <span className="month-grid__date">
                      {fmt.format(day, 'd')}
                    </span>
                    {/*
                       Render *every* event, not just the first three.
                       The visible cell shows the first three plus a
                       "+N more" hint; events past that are still in
                       the DOM but visually clipped via the .sr-only
                       pattern, so the tab navigation hook's
                       aria-activedescendant lookup always finds a
                       real element to point at. Without this an
                       overflow event would have no DOM target and
                       NVDA would fall back to reading "section"
                       instead of the event title.
                     */}
                    {dayEvents.map((ev, evIdx) => {
                      const color = resolveEventColor(
                        ev,
                        calendarById,
                        labelById,
                      );
                      const isFocusedEvent =
                        focused && eventIndex === evIdx;
                      const cal = calendarById.get(ev.calendar_id);
                      const time = ev.all_day
                        ? t('views.allDay')
                        : fmt.format(new Date(ev.start), 'p');
                      const span = multiDayInfo(ev, day);
                      const ariaBase = t('views.week.eventLabel', {
                        title: ev.title,
                        time,
                        calendar: cal?.name ?? '—',
                      });
                      const aria = span
                        ? ariaBase +
                          t('views.multiDaySuffix', {
                            day: span.dayIndex,
                            total: span.totalDays,
                          })
                        : ariaBase;
                      const hidden = evIdx >= visibleLimit;
                      return (
                        <span
                          key={ev.id}
                          id={eventOptionId(flatIndex, evIdx)}
                          className={
                            'month-event' +
                            (isFocusedEvent
                              ? ' month-event--focused'
                              : '') +
                            (hidden ? ' month-event--overflow' : '') +
                            (span ? ' month-event--multiday' : '') +
                            (ev.all_day ? ' month-event--in-lane' : '')
                          }
                          aria-label={aria}
                          aria-selected={isFocusedEvent}
                          onContextMenu={(cmev) => {
                            cmev.preventDefault();
                            cmev.stopPropagation();
                            void openEventMenu(ev);
                          }}
                          style={
                            color.hex
                              ? ({ '--event-color': color.hex } as React.CSSProperties)
                              : undefined
                          }
                        >
                          {ev.title}
                          {span && (
                            <span className="month-event__span">
                              {' '}
                              {t('views.multiDayCompact', {
                                day: span.dayIndex,
                                total: span.totalDays,
                              })}
                            </span>
                          )}
                        </span>
                      );
                    })}
                    {moreCount > 0 && (
                      <span
                        className="month-event month-event--more"
                        aria-hidden="true"
                      >
                        {t('views.month.moreEvents', {
                          count: moreCount,
                        })}
                      </span>
                    )}
                  </div>
                );
              })}
              </div>
            </Fragment>
          );
        })}
      </div>

      <BacklogRail />

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget, 'series');
        }}
        title={t('dialogs.confirm.deleteEventTitle')}
        message={t('dialogs.confirm.deleteEventMessage', {
          title: confirmTarget?.title ?? '',
        })}
      />

      <DeleteEventScopeDialog
        isOpen={scopeTarget !== null}
        onClose={() => setScopeTarget(null)}
        title={scopeTarget?.title ?? ''}
        onOccurrence={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'occurrence');
        }}
        onSeries={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'series');
        }}
      />
    </section>
  );
}

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
  // Local YYYY-MM-DD — see `localDateKey`.
  return localDateKey(d);
}

function groupEventsByDay(events: CalendarEvent[]): Map<string, CalendarEvent[]> {
  const map = new Map<string, CalendarEvent[]>();
  events.forEach((ev) => {
    // Multi-day all-day events appear in every cell they cover — see
    // WeekView.groupEventsByDay for the rationale.
    daysCoveredKeys(ev).forEach((k) => {
      const bucket = map.get(k);
      if (bucket) bucket.push(ev);
      else map.set(k, [ev]);
    });
  });
  return map;
}
