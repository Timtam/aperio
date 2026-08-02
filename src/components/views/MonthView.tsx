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
  isSeriesOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../../intl/recurrence';
import { eventInstanceKey } from '@aperio/shared';
import { useDateFormat } from '../../intl/dateFormat';
import {
  labelsLookup,
  resolveEventColor,
  resolveTaskColor,
} from '../../intl/eventColor';
import {
  buildAllDayBars,
  daysCoveredKeys,
  eventDayTimes,
  multiDayInfo,
} from '../../intl/multiDay';
import {
  expandScheduledRecurringTasks,
  filterTasksOnDay,
  isDeadlineChip,
  isRecurringProjection,
  recurringSeriesTaskId,
} from '../../intl/taskDay';
import { useCurrentUserByList } from '../../state/currentUser';
import {
  assigneeSuffix,
  effortSizeModifier,
  effortSuffix,
  priorityMarker,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskParentSuffix,
  subtaskProgressSuffix,
} from '../../intl/taskStatus';
import { useTaskCascadeEnabled } from '../../state/taskCascadeContext';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useTasks } from '../../state/useTasks';
import { useTaskListShowCompleted } from '../../state/useTaskListShowCompleted';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange, type WeekStart } from '../../state/viewMath';
import type { CalendarEvent, Task } from '../../api/types';
import { BacklogRail } from '../BacklogRail';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';
import { deleteThisAndFuture } from '../../state/deleteSeriesFromOccurrence';
import {
  EVENT_DND_TYPE,
  moveEventToDay,
  readEventDrag,
  readTaskDrag,
  scheduleTaskOnDay,
  setEventDrag,
  setTaskDrag,
  TASK_DND_TYPE,
  type MoveCopyScope,
} from '../../state/moveActions';
import { MoveEventScopeDialog } from '../MoveEventScopeDialog';

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

/** A chip in a month-grid day cell: an event or a task scheduled/due that
 *  day. The discriminated union lets the render loop, the focus index and
 *  the aria-activedescendant keyboard nav agree on which chip is focused —
 *  mirroring WeekView's `DayItem` so Tab walks events *and* tasks. */
type MonthDayItem =
  | { kind: 'event'; id: string; title: string; event: CalendarEvent }
  | { kind: 'task'; id: string; title: string; task: Task; isBy: boolean };

export function MonthView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor, setAnchor, goPrev, goNext, weekStartsOn } = useViewState();
  const { openEventDialog, openTaskDialog, openCreateChooser, invalidateData } =
    useDialogState();
  const { openForEvent: openEventMenu, openForTask: openTaskMenu } =
    useChipContextMenu();

  const cells = useMemo(
    () => buildMonthGrid(anchor, weekStartsOn),
    [anchor, weekStartsOn],
  );
  const range = useMemo(() => visibleRange('month', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { tasks, taskListById } = useTasks();
  const { visualEffortSizing } = useTaskCascadeEnabled();
  const currentUserByList = useCurrentUserByList(tasks);
  // Hide tasks assigned to a concrete OTHER user from MY calendar (mine +
  // unassigned stay) — the day-start review's ownership filter (DESIGN §9.7).
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );
  const toggleTaskStatus = useTaskStatusToggle();
  const { shouldShow: shouldShowCompletedForList } = useTaskListShowCompleted();
  const { colorLabels, sectionColorById, sectionsByList, loadSections } =
    useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  // Load sections for the lists with tasks so a colored section cascades to
  // its tasks here too (cached + cheap; empty for section-less backends).
  // Mirrors WeekView / TaskView.
  const listIdsWithTasks = useMemo(
    () => Array.from(new Set(tasks.map((task) => task.list_id))),
    [tasks],
  );
  useEffect(() => {
    for (const listId of listIdsWithTasks) {
      if (!(listId in sectionsByList)) void loadSections(listId);
    }
  }, [listIdsWithTasks, sectionsByList, loadSections]);

  const eventsByDay = useMemo(() => groupEventsByDay(events), [events]);

  // Expand recurring SCHEDULED tasks into one occurrence per planned day across
  // the visible month grid — so a task recurring every day/week shows on EVERY
  // due day (like a recurring event), not only its single current
  // scheduled_date. The occurrence on the task's own date is the real,
  // interactive task; the others are read-only projections (isRecurringProjection)
  // that route to the series and offer no complete/reschedule/delete (the current
  // instance advances the series on completion). Non-recurring / from-completion /
  // backlog tasks pass through untouched. Keyed by the grid's covered range so a
  // series recurring into an adjacent-month padding cell still shows there.
  const expandedTasks = useMemo(() => {
    if (cells.length === 0) return tasks;
    let fromKey = keyOf(cells[0]);
    let toKey = fromKey;
    for (const c of cells) {
      const k = keyOf(c);
      if (k < fromKey) fromKey = k;
      if (k > toKey) toKey = k;
    }
    return expandScheduledRecurringTasks(tasks, fromKey, toKey);
  }, [tasks, cells]);

  // Resolve a (possibly projected) task back to its real, interactive series
  // task so opening/activating a projection opens the underlying task, never a
  // non-existent occurrence id. A no-op for a real task.
  const seriesTaskOf = useCallback(
    (task: Task): Task => {
      if (!isRecurringProjection(task)) return task;
      const id = recurringSeriesTaskId(task.id);
      return tasks.find((x) => x.id === id) ?? task;
    },
    [tasks],
  );

  // Per-day items for the cells + keyboard nav: the day's events followed by
  // the tasks scheduled or due that day (a "due" task carries `isBy` for the
  // deadline ring). A discriminated union so the render loop, focus index and
  // aria-activedescendant all agree on which chip is which — mirroring
  // WeekView's `DayItem` so Tab walks events *and* tasks in the month grid.
  const itemsByDay = useMemo(() => {
    const map = new Map<string, MonthDayItem[]>();
    for (const cell of cells) {
      const key = keyOf(cell);
      const dayEvents = eventsByDay.get(key) ?? [];
      const toEventItem = (event: CalendarEvent): MonthDayItem => ({
        kind: 'event',
        id: event.id,
        title: event.title,
        event,
      });
      const taskItems: MonthDayItem[] = filterTasksOnDay(
        expandedTasks,
        key,
        shouldShowCompletedForList,
        meFor,
      ).map((task) => ({
        kind: 'task',
        id: `task-${task.id}`,
        title: task.title,
        task,
        isBy: isDeadlineChip(task, key),
      }));
      // Order: timed events, then tasks, then all-day events. All-day events
      // render hidden inside the cell (they live in the lane above the row)
      // but stay in the DOM for keyboard nav — keeping them last leaves the
      // cell's visible slots for the timed events and tasks the user actually
      // sees there, so a task is never starved by a hidden all-day chip.
      map.set(key, [
        ...dayEvents.filter((event) => !event.all_day).map(toEventItem),
        ...taskItems,
        ...dayEvents.filter((event) => event.all_day).map(toEventItem),
      ]);
    }
    return map;
  }, [cells, eventsByDay, expandedTasks, shouldShowCompletedForList, meFor]);

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
    () => cells.map((d) => ({ items: itemsByDay.get(keyOf(d)) ?? [] })),
    [cells, itemsByDay],
  );
  const focusedDayItems = useMemo(
    () => buckets[focusIndex]?.items ?? [],
    [buckets, focusIndex],
  );

  const dayChangeAnnouncer = useCallback(
    (newDayIdx: number, item: MonthDayItem) => {
      announce(
        t('views.month.tabAnnounce', {
          day: fmt.format(cells[newDayIdx], 'PPPP'),
          title: item.title,
        }),
      );
    },
    [announce, cells, fmt, t],
  );

  const {
    eventIndex,
    clear: clearEventIndex,
    handleTab,
  } = useEventTabNavigation<MonthDayItem>({
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
  // Source task being dragged (to reschedule onto another day, or back to the
  // backlog rail which unschedules it). Drives the dimming class on the chip.
  const [draggingTaskId, setDraggingTaskId] = useState<string | null>(null);

  const performDelete = useCallback(
    async (
      ev: CalendarEvent,
      scope: 'occurrence' | 'this_and_future' | 'series',
      sendCancellations = false,
    ) => {
      try {
        const occIso = occurrenceIsoOf(ev);
        if (scope === 'occurrence' && occIso) {
          await addEventExdate(seriesIdOf(ev), occIso, ev.calendar_id, sendCancellations);
          announce(
            t(
              sendCancellations
                ? 'dialogs.event.occurrenceCancelled'
                : 'dialogs.event.occurrenceDeleted',
              { title: ev.title },
            ),
          );
        } else if (scope === 'this_and_future' && occIso) {
          await deleteThisAndFuture(ev, occIso, sendCancellations);
          announce(
            t(
              sendCancellations
                ? 'dialogs.event.thisAndFutureCancelled'
                : 'dialogs.event.thisAndFutureDeleted',
              { title: ev.title },
            ),
          );
        } else {
          await deleteEventById(seriesIdOf(ev), ev.calendar_id, sendCancellations);
          announce(
            t(
              sendCancellations
                ? 'dialogs.event.meetingCancelled'
                : 'dialogs.event.deleted',
              { title: ev.title },
            ),
          );
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
    // Only an expanded occurrence has a single instance to delete; a bare
    // recurring master row (unexpandable RRULE) has none, so it takes the plain
    // confirm (series delete) instead of a scope choice that would otherwise fall
    // through to deleting the whole series.
    if (isSeriesOccurrence(ev)) {
      setScopeTarget(ev);
    } else {
      setConfirmTarget(ev);
    }
  }, []);

  // Event chip dropped on a day cell → move it there (time + duration
  // stay). Recurring events first ask for the §7.5 scope ("only this
  // occurrence / whole series") via the pending state + dialog below.
  const [pendingEventDrop, setPendingEventDrop] = useState<{
    event: CalendarEvent;
    dayKey: string;
  } | null>(null);
  const performEventDrop = useCallback(
    async (ev: CalendarEvent, dayKey: string, scope: MoveCopyScope) => {
      try {
        const moved = await moveEventToDay(ev, dayKey, scope);
        if (!moved) return; // same-day drop — nothing to announce
        announce(
          t('views.eventMovedToDay', {
            title: ev.title,
            date: fmt.format(new Date(`${dayKey}T00:00:00`), 'PPP'),
          }),
        );
        invalidateData();
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [announce, t, fmt, invalidateData],
  );

  // Drag-and-drop: a task dropped on a day cell is scheduled on that day
  // (e.g. dragged out of the backlog rail); an event dropped on a day
  // cell moves there. Mouse affordance only — the keyboard/SR paths are
  // the task plan dialog and the event editor.
  const scheduleByDrop = useCallback(
    async (day: Date, e: React.DragEvent) => {
      e.preventDefault();
      const dayKey = localDateKey(day);
      const payload = readTaskDrag(e.dataTransfer);
      if (!payload) {
        const dropped = readEventDrag(e.dataTransfer);
        if (!dropped) return;
        if (isSeriesOccurrence(dropped) || dropped.recurrence?.rrule) {
          setPendingEventDrop({ event: dropped, dayKey });
          return;
        }
        await performEventDrop(dropped, dayKey, 'series');
        return;
      }
      if (payload.task.scheduled_date === dayKey) return;
      try {
        await scheduleTaskOnDay(payload.task, dayKey);
        invalidateData();
        announce(
          t('views.backlog.scheduled', {
            title: payload.task.title,
            date: fmt.format(day, 'PPP'),
          }),
        );
      } catch (err) {
        announce(
          isCommandError(err) ? `${err.code}: ${err.message}` : String(err),
        );
      }
    },
    [invalidateData, announce, t, fmt, performEventDrop],
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
        const item = focusedDayItems[eventIndex];
        if (e.key === 'Escape') {
          e.preventDefault();
          clearEventIndex();
          return;
        }
        if (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          if (item?.kind === 'event') {
            openEventDialog(item.event);
          } else if (item?.kind === 'task') {
            // Match TaskView / WeekView: Enter opens the task, Space ticks
            // it off (the visible ○/● marker). A read-only recurring projection
            // opens its series on Enter and ignores Space (its completion lives
            // on the current instance).
            if (e.key === 'Enter') openTaskDialog(seriesTaskOf(item.task));
            else if (!isRecurringProjection(item.task)) void toggleTaskStatus(item.task);
          }
          return;
        }
        if (e.key === 'Delete' || e.key === 'Backspace') {
          e.preventDefault();
          // Only events are deletable from the grid; a task is managed via
          // its dialog / context menu.
          if (item?.kind === 'event') requestDelete(item.event);
          return;
        }
        if (
          e.key === 'ContextMenu' ||
          (e.shiftKey && e.key === 'F10')
        ) {
          e.preventDefault();
          if (item) {
            const target = e.currentTarget as HTMLElement;
            const id = eventOptionId(focusIndex, eventIndex);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            const pos = rect
              ? { x: rect.left, y: rect.bottom }
              : undefined;
            if (item.kind === 'event') void openEventMenu(item.event, pos);
            // A projection has no context actions (complete/reschedule/delete
            // live on the current instance) — the menu is suppressed for it.
            else if (!isRecurringProjection(item.task)) void openTaskMenu(item.task, pos);
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
          setAnchor(startOfWeek(anchor, { weekStartsOn }));
          return;
        }
        case 'End': {
          e.preventDefault();
          setAnchor(endOfWeek(anchor, { weekStartsOn }));
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
          const items = itemsByDay.get(keyOf(focusedDay)) ?? [];
          const first = items[0];
          if (first?.kind === 'event') {
            openEventDialog(first.event);
          } else if (first?.kind === 'task') {
            openTaskDialog(seriesTaskOf(first.task));
          } else {
            // Empty day → the "Termin oder Aufgabe?" chooser, anchored here.
            openCreateChooser(keyOf(focusedDay));
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
      weekStartsOn,
      cells,
      focusIndex,
      eventIndex,
      focusedDayItems,
      itemsByDay,
      openEventDialog,
      openTaskDialog,
      openCreateChooser,
      toggleTaskStatus,
      seriesTaskOf,
      handleTab,
      clearEventIndex,
      requestDelete,
      openEventMenu,
      openTaskMenu,
      eventOptionId,
    ],
  );

  const today = useMemo(() => new Date(), []);
  const rowCount = cells.length / 7;
  // Size every week-row to the BUSIEST day so all cells stay a uniform calendar
  // grid AND no events clip; the grid scrolls when that exceeds the window. The
  // count drives a rem-based row min-height in CSS, so it tracks the UI
  // font-size without a DOM measurement.
  // Effort-sized task chips are TALLER than the 1.4rem/item budget assumes
  // (`.month-task--effort-medium` 1.9em / `--effort-large` 2.6em plus their
  // padding at the cell font), so they count as more than one unit — without
  // the weight, a day dense with large-effort tasks would outgrow its row's
  // min-height and break the uniform month grid the budget exists for.
  const itemUnits = useCallback(
    (item: MonthDayItem): number => {
      if (!visualEffortSizing || item.kind !== 'task') return 1;
      if (item.task.effort === 'medium') return 1.6;
      if (item.task.effort === 'large') return 2.2;
      return 1;
    },
    [visualEffortSizing],
  );
  const maxItems = useMemo(
    () =>
      buckets.reduce(
        (m, b) => Math.max(m, b.items.reduce((n, it) => n + itemUnits(it), 0)),
        0,
      ),
    [buckets, itemUnits],
  );
  // Uniform row height — overhead + the busiest day's item units, in rem so it
  // scales with the UI font-size. Set INLINE on every row so they're all
  // identical (and so it can't be lost to var inheritance / calc resolution).
  const rowMinHeight = `${(2 + maxItems * 1.4).toFixed(2)}rem`;
  const gridRef = useAutoFocus<HTMLDivElement>(!loading);

  // The month grid scrolls (see `.month-grid` in styles.css): each week row
  // grows to fit its busiest day, so EVERY event is shown rather than capped to
  // a measured cell height (the old px-based budget didn't track the UI
  // font-size and clipped events with no way to scroll to them). No per-cell
  // limit — the render's "+N more" path stays but never triggers.
  const visiblePerCell = Number.POSITIVE_INFINITY;

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

      <div className="view__body">
        <BacklogRail />
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
                    // Sighted-only affordance: the all-day lane is a drop target
                    // so a bar can be dragged sideways to another day (its natural
                    // gesture — the day cells below only catch drops in their own
                    // body). A drop over another bar bubbles here; the target day
                    // is read from the cursor X against this row's cells. SR users
                    // move all-day events via the Move/Copy dialog instead.
                    onDragOver={(e) => {
                      const types = e.dataTransfer.types;
                      if (
                        !types.includes(TASK_DND_TYPE) &&
                        !types.includes(EVENT_DND_TYPE)
                      ) {
                        return;
                      }
                      e.preventDefault();
                      e.dataTransfer.dropEffect = 'move';
                    }}
                    onDrop={(e) => {
                      for (let col = 0; col < rowCells.length; col += 1) {
                        const cellEl = document.getElementById(
                          cellId(row * 7 + col),
                        );
                        if (!cellEl) continue;
                        const r = cellEl.getBoundingClientRect();
                        if (e.clientX >= r.left && e.clientX < r.right) {
                          void scheduleByDrop(rowCells[col], e);
                          return;
                        }
                      }
                    }}
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
                          key={eventInstanceKey(bar.event)}
                          className={
                            'month-allday-bar' +
                            (isBarFocused
                              ? ' month-allday-bar--focused'
                              : '') +
                            (bar.event.cancelled
                              ? ' month-allday-bar--cancelled'
                              : '') +
                            (bar.continuesBefore
                              ? ' month-allday-bar--continues-before'
                              : '') +
                            (bar.continuesAfter
                              ? ' month-allday-bar--continues-after'
                              : '')
                          }
                          style={style}
                          // The bar is the all-day event's only VISIBLE
                          // representation (the in-cell chips are clipped
                          // a11y anchors) — so it must be the drag source
                          // for day-/calendar-moves too. Mouse-only; SR
                          // paths go through the chip + dialogs.
                          draggable
                          onDragStart={(dev) => {
                            setEventDrag(dev.dataTransfer, bar.event);
                          }}
                          onDoubleClick={(e) => {
                            e.stopPropagation();
                            openEventDialog(bar.event);
                          }}
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
                <div
                  role="row"
                  className="month-grid__row"
                  style={{ minHeight: rowMinHeight }}
                >
                <div role="rowheader" className="month-grid__kw">
                  {fmt.isoWeek(addDays(rowStart, 3))}
                </div>
                {cells.slice(row * 7, row * 7 + 7).map((day, col) => {
                  const flatIndex = row * 7 + col;
                  const dayItems = itemsByDay.get(keyOf(day)) ?? [];
                  const focused = flatIndex === focusIndex;
                  const outside = !isSameMonth(day, anchor);
                  // When there are more chips (events + tasks) than fit,
                  // reserve the last visible slot for the "+N more" hint so the
                  // cell never overflows its row.
                  const visibleLimit =
                    dayItems.length > visiblePerCell
                      ? Math.max(0, visiblePerCell - 1)
                      : visiblePerCell;
                  const moreCount = dayItems.length - visibleLimit;
                  return (
                    <div
                      key={day.toISOString()}
                      id={cellId(flatIndex)}
                      role="gridcell"
                      aria-selected={focused}
                      aria-current={isSameDay(day, today) ? 'date' : undefined}
                      aria-label={t('views.month.dayAnnounce', {
                        day: fmt.format(day, 'PPPP'),
                        count: dayItems.length,
                      })}
                      className={
                        'month-grid__cell' +
                        (focused ? ' month-grid__cell--focused' : '') +
                        (outside ? ' month-grid__cell--outside' : '') +
                        (isSameDay(day, today) ? ' month-grid__cell--today' : '')
                      }
                      onClick={() => setAnchor(day)}
                      onDoubleClick={(e) => {
                        // Double-click on an empty part of the day opens the
                        // "Termin oder Aufgabe?" chooser, anchored to it. Skip
                        // clicks that land on a chip (events/tasks are draggable
                        // and have their own double-click → editor). Keyboard
                        // equivalent: Enter on the focused day (handleKeyDown).
                        if (
                          (e.target as HTMLElement).closest('[draggable="true"]')
                        ) {
                          return;
                        }
                        openCreateChooser(keyOf(day));
                      }}
                      onDragOver={(e) => {
                        const types = e.dataTransfer.types;
                        if (
                          !types.includes(TASK_DND_TYPE) &&
                          !types.includes(EVENT_DND_TYPE)
                        ) {
                          return;
                        }
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
                      {dayItems.map((item, idx) => {
                        const isFocusedItem = focused && eventIndex === idx;
                        const hidden = idx >= visibleLimit;
                        if (item.kind === 'task') {
                          const task = item.task;
                          // A read-only future occurrence of a recurring task —
                          // rendered as a preview: no drag/complete/menu; it
                          // opens its series on activate.
                          const projection = isRecurringProjection(task);
                          const color = resolveTaskColor(
                            task,
                            taskListById,
                            labelById,
                            sectionColorById,
                          );
                          const priorityGlyph = priorityMarker(task.priority);
                          const effortMod = visualEffortSizing
                            ? effortSizeModifier(task.effort)
                            : '';
                          // Visible deadline badge on the SCHEDULED chip (a
                          // deadline-only marker, `isBy`, already sits on its
                          // own day).
                          const deadlineBadge =
                            !item.isBy && task.deadline_date
                              ? fmt.format(
                                  new Date(`${task.deadline_date}T00:00:00`),
                                  'P',
                                )
                              : '';
                          // The scheduled chip now announces its deadline too
                          // (the deadline-day duplicate is suppressed), so use
                          // the "fällig bis …" label whenever there's a deadline.
                          const aria =
                            t(
                              task.deadline_date
                                ? 'views.week.taskChipBy'
                                : 'views.week.taskChip',
                              {
                                title: task.title,
                                deadline: task.deadline_date
                                  ? fmt.format(
                                      new Date(`${task.deadline_date}T00:00:00`),
                                      'PPP',
                                    )
                                  : '',
                                state: t(statusI18nKey(task.status)),
                                priority: prioritySuffix(t, task.priority),
                                progress: subtaskProgressSuffix(
                                  t,
                                  task.id,
                                  tasks,
                                ),
                                assignee: assigneeSuffix(t, task.assignees),
                              },
                            ) +
                            subtaskParentSuffix(t, task, tasks) +
                            effortSuffix(t, task.effort) +
                            (projection
                              ? t('views.tasks.recurringOccurrence')
                              : '');
                          return (
                            <span
                              key={item.id}
                              id={eventOptionId(flatIndex, idx)}
                              className={
                                'month-event month-task' +
                                (isFocusedItem
                                  ? ' month-event--focused'
                                  : '') +
                                (hidden ? ' month-event--overflow' : '') +
                                (item.isBy ? ' month-task--by' : '') +
                                (projection
                                  ? ' month-task--projection'
                                  : '') +
                                (draggingTaskId === task.id
                                  ? ' month-task--dragging'
                                  : '') +
                                ` month-task--${task.status.replace('_', '-')}` +
                                (effortMod
                                  ? ` month-task--effort-${effortMod}`
                                  : '')
                              }
                              aria-label={aria}
                              aria-selected={isFocusedItem}
                              // A projection is read-only: not draggable, and it
                              // opens its series (never toggles/menus) on activate.
                              draggable={!projection}
                              onDragStart={
                                projection
                                  ? undefined
                                  : (dev) => {
                                      setTaskDrag(
                                        dev.dataTransfer,
                                        task,
                                        tasks.filter((c) => c.parent_id === task.id),
                                      );
                                      setDraggingTaskId(task.id);
                                    }
                              }
                              onDragEnd={
                                projection ? undefined : () => setDraggingTaskId(null)
                              }
                              onDoubleClick={(e) => {
                                e.stopPropagation();
                                openTaskDialog(seriesTaskOf(task));
                              }}
                              onContextMenu={(cmev) => {
                                cmev.preventDefault();
                                cmev.stopPropagation();
                                if (!projection) void openTaskMenu(task);
                              }}
                              style={
                                color.hex
                                  ? ({ '--event-color': color.hex } as React.CSSProperties)
                                  : undefined
                              }
                            >
                              <span
                                className="month-task__check"
                                aria-hidden="true"
                                onClick={
                                  projection
                                    ? undefined
                                    : (cmev) => {
                                        cmev.stopPropagation();
                                        void toggleTaskStatus(task);
                                      }
                                }
                              >
                                {projection ? '↻' : statusMarker(task.status)}
                              </span>
                              <span className="month-task__title">
                                {task.parent_id ? '↳ ' : ''}
                                {task.title}
                              </span>
                              {priorityGlyph && (
                                <span
                                  className="month-task__priority"
                                  aria-hidden="true"
                                >
                                  {priorityGlyph}
                                </span>
                              )}
                              {deadlineBadge && (
                                <span
                                  className="month-task__deadline"
                                  aria-hidden="true"
                                >
                                  {t('views.week.taskChipDeadlineBadge', {
                                    deadline: deadlineBadge,
                                  })}
                                </span>
                              )}
                            </span>
                          );
                        }
                        const ev = item.event;
                        const color = resolveEventColor(
                          ev,
                          calendarById,
                          labelById,
                        );
                        const cal = calendarById.get(ev.calendar_id);
                        const span = multiDayInfo(ev, day);
                        // MonthView shows only the START time. For a TIMED event
                        // that crosses midnight (`span` non-null) show this day's
                        // CLAMPED start — so the next-day tail cell reads "00:00",
                        // not the misleading absolute "23:00". Single-day timed
                        // events keep the absolute start. All-day → "all day".
                        const time = ev.all_day
                          ? t('views.allDay')
                          : span
                            ? eventDayTimes(fmt, ev, day).startStr
                            : fmt.format(new Date(ev.start), 'p');
                        // The continuation (tail) chip of a TIMED cross-midnight
                        // event must NOT be draggable: `moveEventToDay` derives
                        // the move delta from the absolute START day, so dragging
                        // the day-N+1 chip would reschedule relative to day N —
                        // wrong. The start-day chip (dayIndex 1) keeps a
                        // well-defined anchor and stays draggable. (Blind users
                        // use Move/Copy; this only fixes the mouse affordance.)
                        // All-day multi-day chips keep dragging unchanged.
                        const isTimedTail =
                          !ev.all_day && span != null && span.dayIndex > 1;
                        const ariaBase = t('views.week.eventLabel', {
                          title: ev.title,
                          time,
                          calendar: cal?.name ?? '—',
                        });
                        const aria =
                          (span
                            ? ariaBase +
                              t('views.multiDaySuffix', {
                                day: span.dayIndex,
                                total: span.totalDays,
                              })
                            : ariaBase) +
                          (ev.cancelled ? t('views.eventCancelledSuffix') : '');
                        return (
                          <span
                            key={eventInstanceKey(ev)}
                            id={eventOptionId(flatIndex, idx)}
                            className={
                              'month-event' +
                              (isFocusedItem
                                ? ' month-event--focused'
                                : '') +
                              (ev.cancelled ? ' month-event--cancelled' : '') +
                              (hidden ? ' month-event--overflow' : '') +
                              (span ? ' month-event--multiday' : '') +
                              (ev.all_day ? ' month-event--in-lane' : '')
                            }
                            aria-label={aria}
                            aria-selected={isFocusedItem}
                            draggable={!isTimedTail}
                            onDragStart={
                              isTimedTail
                                ? undefined
                                : (dev) => {
                                    // Drag onto another day cell to move the
                                    // event there, or onto a sidebar calendar
                                    // row to move it to that calendar (mouse
                                    // affordance; keyboard/SR paths are the
                                    // editor + Move/Copy dialog).
                                    setEventDrag(dev.dataTransfer, ev);
                                  }
                            }
                            onDoubleClick={(dcev) => {
                              // Open the editor, mirroring the task chips
                              // (the keyboard path is Enter on the focused
                              // item). Single click stays free for the
                              // cell's day-anchor selection.
                              dcev.stopPropagation();
                              openEventDialog(ev);
                            }}
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
      </div>

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
        event={scopeTarget}
        onOccurrence={(send) => {
          if (scopeTarget) void performDelete(scopeTarget, 'occurrence', send);
        }}
        onThisAndFuture={(send) => {
          if (scopeTarget)
            void performDelete(scopeTarget, 'this_and_future', send);
        }}
        onSeries={(send) => {
          if (scopeTarget) void performDelete(scopeTarget, 'series', send);
        }}
      />
      <MoveEventScopeDialog
        isOpen={pendingEventDrop !== null}
        onClose={() => setPendingEventDrop(null)}
        title={pendingEventDrop?.event.title ?? ''}
        onOccurrence={() => {
          if (pendingEventDrop) {
            void performEventDrop(
              pendingEventDrop.event,
              pendingEventDrop.dayKey,
              'occurrence',
            );
          }
        }}
        onSeries={() => {
          if (pendingEventDrop) {
            void performEventDrop(
              pendingEventDrop.event,
              pendingEventDrop.dayKey,
              'series',
            );
          }
        }}
      />
    </section>
  );
}

function buildMonthGrid(anchor: Date, weekStartsOn: WeekStart): Date[] {
  const first = startOfMonth(anchor);
  const last = endOfMonth(anchor);
  const gridStart = startOfWeek(first, { weekStartsOn });
  const gridEnd = endOfWeek(last, { weekStartsOn });
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
    // Multi-day all-day events AND timed events that cross midnight appear in
    // every cell they cover (via daysCoveredKeys) — a 23:00→01:00 meeting lands
    // on both its start cell and the next. See WeekView.groupEventsByDay for the
    // rationale.
    daysCoveredKeys(ev).forEach((k) => {
      const bucket = map.get(k);
      if (bucket) bucket.push(ev);
      else map.set(k, [ev]);
    });
  });
  return map;
}
