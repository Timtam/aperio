import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { addDays, isSameDay, startOfWeek } from 'date-fns';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../../a11y/announcerContext';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useEventTabNavigation } from '../../hooks/useEventTabNavigation';
import { localDateKey } from '../../intl/dateKey';
import { useDateFormat } from '../../intl/dateFormat';
import {
  labelsLookup,
  resolveEventColor,
  resolveTaskColor,
} from '../../intl/eventColor';
import {
  buildAllDayBars,
  daysCoveredKeys,
  multiDayInfo,
} from '../../intl/multiDay';
import { useCalendarStore } from '../../state/calendarStoreContext';
import {
  EVENT_DND_TYPE,
  moveEventToDay,
  readEventDrag,
  setEventDrag,
  setTaskDrag,
  TASK_DND_TYPE,
  type MoveCopyScope,
} from '../../state/moveActions';
import { isExpandedOccurrence } from '../../intl/recurrence';
import { MoveEventScopeDialog } from '../MoveEventScopeDialog';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useTaskListShowCompleted } from '../../state/useTaskListShowCompleted';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange } from '../../state/viewMath';
import {
  groupTasksByDay,
  isDeadlineChip,
  mergeDayItems,
  taskTimeOnDay,
} from '../../intl/taskDay';
import { useCurrentUserByList } from '../../state/currentUser';
import {
  assigneeSuffix,
  priorityMarker,
  prioritySuffix,
  statusI18nKey,
  statusMarker,
  subtaskParentSuffix,
  subtaskProgressSuffix,
} from '../../intl/taskStatus';
import type { CalendarEvent, Task } from '../../api/types';
import { BacklogRail } from '../BacklogRail';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';

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
  const announce = useAnnouncer();
  const { anchor, setAnchor, goPrev, goNext, weekStartsOn } = useViewState();
  const { openEventDialog, openTaskDialog, invalidateData } =
    useDialogState();

  const range = useMemo(
    () => visibleRange('week', anchor, weekStartsOn),
    [anchor, weekStartsOn],
  );
  const { events, calendarById, loading } = useEvents(range);
  const { tasks, taskListById } = useTasks();
  const currentUserByList = useCurrentUserByList(tasks);
  // Hide tasks assigned to a concrete OTHER user from MY calendar (mine +
  // unassigned stay) — the day-start review's ownership filter (DESIGN §9.7).
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );
  const toggleTaskStatus = useTaskStatusToggle();
  const { shouldShow: shouldShowCompletedForList } =
    useTaskListShowCompleted();
  const { openForEvent: openEventMenu, openForTask: openTaskMenu } =
    useChipContextMenu();
  const { colorLabels, sectionColorById, sectionsByList, loadSections } =
    useCalendarStore();

  // Load sections for the lists with tasks here so a colored section
  // cascades to its tasks in this view too (cached + cheap; empty for
  // section-less backends). Mirrors TaskView.
  const listIdsWithTasks = useMemo(
    () => Array.from(new Set(tasks.map((task) => task.list_id))),
    [tasks],
  );
  useEffect(() => {
    for (const listId of listIdsWithTasks) {
      if (!(listId in sectionsByList)) void loadSections(listId);
    }
  }, [listIdsWithTasks, sectionsByList, loadSections]);
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  const weekStart = useMemo(
    () => startOfWeek(anchor, { weekStartsOn }),
    [anchor, weekStartsOn],
  );
  const days = useMemo(
    () => Array.from({ length: 7 }, (_, i) => addDays(weekStart, i)),
    [weekStart],
  );

  const eventsByDay = useMemo(
    () => groupEventsByDay(events, days),
    [events, days],
  );

  // Bucket tasks per visible day (§9.4). A task lands on a day when it
  // is scheduled for that day OR due (deadline) that day — the deadline
  // shows as a point marker on its deadline day, not a span across every
  // day until then (see `filterTasksOnDay`). A task scheduled on one day
  // and due on another therefore appears on both; same-day collapses to
  // a single chip.
  const tasksByDay = useMemo(() => {
    const dayKeys = days.map((d) => keyOf(d));
    return groupTasksByDay(tasks, dayKeys, shouldShowCompletedForList, meFor);
  }, [tasks, days, shouldShowCompletedForList, meFor]);

  // Pre-merge each day's events + timed tasks into a single time-sorted
  // list, then split that list back into timed and untimed buckets.
  // The exact same `timed` array drives:
  //   - the Tab-navigation buckets (so Tab walks events *and* timed
  //     tasks chronologically across the visible week);
  //   - the cell rendering (so the visual order, the focus index, and
  //     `aria-activedescendant` all agree on which chip is which).
  const dayItemsByDay = useMemo(() => {
    const map = new Map<
      string,
      ReturnType<typeof mergeDayItems<CalendarEvent, Task>>
    >();
    for (const day of days) {
      const dayKey = keyOf(day);
      map.set(
        dayKey,
        mergeDayItems(
          eventsByDay.get(dayKey) ?? [],
          tasksByDay.get(dayKey) ?? [],
          dayKey,
          (ev) => new Date(ev.start).getTime(),
        ),
      );
    }
    return map;
  }, [days, eventsByDay, tasksByDay]);

  // Build the all-day lane bars over the week. The lane is the
  // visual half of variant B: a contiguous strip above the day cells
  // where each multi-day all-day event spans the columns it covers.
  // SR users still find the underlying event via the per-day chips
  // inside the cells (those carry the listbox options) — the lane
  // here is `aria-hidden` and exists only for sighted users.
  const allDayBars = useMemo(
    () => buildAllDayBars(events, days),
    [events, days],
  );
  const laneRows = allDayBars.reduce((m, b) => Math.max(m, b.lane + 1), 0);

  const focusIndex = useMemo(() => {
    const i = days.findIndex((d) => isSameDay(d, anchor));
    return i >= 0 ? i : 0;
  }, [days, anchor]);

  // Unique prefix per WeekView instance, in case there's ever more than
  // one on screen (e.g. a future side-by-side comparison).
  const idPrefix = useId();
  const cellId = (i: number) => `${idPrefix}-cell-${i}`;
  const eventOptionId = useCallback(
    (dayIdx: number, evIdx: number) =>
      `${idPrefix}-cell-${dayIdx}-ev-${evIdx}`,
    [idPrefix],
  );

  // Two-level focus: `null` means the day cell itself is focused (arrow
  // keys move the day). A number means the user has tabbed into the
  // day and is focused on the n-th item of that day — Enter opens it,
  // Delete removes events, Escape returns to the day cell.
  //
  // Tab is handled by the shared hook below: it crosses day boundaries
  // and moves the anchor for us, so the visual day selection follows
  // the focused item the way it does in Outlook. Tasks with a concrete
  // deadline_time live in the same focus order as events — that's the
  // a11y half of the §9.4 interleave fix.
  type DayItem =
    | { kind: 'event'; event: CalendarEvent; title: string }
    | { kind: 'task'; task: Task; title: string };
  const buckets = useMemo<{ items: DayItem[] }[]>(
    () =>
      days.map((d) => {
        const merged = dayItemsByDay.get(keyOf(d));
        const timed: DayItem[] =
          merged?.timed.map((m) =>
            m.kind === 'event'
              ? {
                  kind: 'event' as const,
                  event: m.event,
                  title: m.event.title,
                }
              : {
                  kind: 'task' as const,
                  task: m.task,
                  title: m.task.title,
                },
          ) ?? [];
        // Untimed tasks join the SAME nav buckets, after the timed lane.
        // The grid traps Tab (handleTab wraps within the buckets and never
        // lets focus escape), so anything *not* in a bucket is keyboard-
        // unreachable. WeekDayTasks renders these below the lane and mirrors
        // their indices (timed count + offset) for aria-activedescendant.
        const untimed: DayItem[] =
          merged?.untimed.map((task) => ({
            kind: 'task' as const,
            task,
            title: task.title,
          })) ?? [];
        return { items: [...timed, ...untimed] };
      }),
    [days, dayItemsByDay],
  );
  const focusedDayItems = useMemo(
    () => buckets[focusIndex]?.items ?? [],
    [buckets, focusIndex],
  );

  const dayChangeAnnouncer = useCallback(
    (newDayIdx: number, item: DayItem) => {
      announce(
        t('views.week.tabAnnounce', {
          day: fmt.format(days[newDayIdx], 'PPPP'),
          title: item.title,
        }),
      );
    },
    [announce, days, fmt, t],
  );

  const {
    eventIndex,
    clear: clearEventIndex,
    handleTab,
  } = useEventTabNavigation<DayItem>({
    buckets,
    dayIndex: focusIndex,
    setDayIndex: (next) => setAnchor(days[next]),
    onDayChange: dayChangeAnnouncer,
  });

  // The currently focused event (if any) drives the lane bar's
  // focused-state styling: when a per-day chip of a multi-day event
  // is the active descendant, the bar above lights up — keeps visual
  // focus on the thing the user actually sees. Tasks have no
  // companion lane bar, so the lookup deliberately returns `null` on
  // task items.
  const focusedItem =
    eventIndex !== null
      ? (buckets[focusIndex]?.items[eventIndex] ?? null)
      : null;
  const focusedEvId =
    focusedItem?.kind === 'event' ? focusedItem.event.id : null;

  // Delete confirmation. Non-recurring events go through the
  // straight Confirm dialog; recurring occurrences need a scope
  // choice ("only this one" vs "the whole series"), so they get
  // their own three-button dialog.
  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const [scopeTarget, setScopeTarget] = useState<CalendarEvent | null>(null);

  // Drag-and-drop rescheduling (§ 9.4 "Drag & Drop auf Wochentage"):
  // a task chip can be dragged from its current day onto another day
  // in the visible week to flip its `scheduled_date`. Mouse-only —
  // keyboard / SR users have Shift+D (the PlanTaskDialog) for the
  // same outcome.
  //
  //   - `draggingTaskId`: which task is currently being dragged.
  //     Used by the cell drop-handlers as a "is this an Aperio task
  //     drag worth reacting to" check, since `dataTransfer.getData`
  //     during `dragover` is restricted to a few MIME types in
  //     modern browsers — we keep the id in component state instead.
  //   - `dragOverDayKey`: which day cell is currently the hovered
  //     drop target. Drives the highlight class on the cell.
  const [draggingTaskId, setDraggingTaskId] = useState<string | null>(null);
  const [dragOverDayKey, setDragOverDayKey] = useState<string | null>(null);

  const rescheduleTaskByDrop = useCallback(
    async (taskId: string, newDayKey: string) => {
      const task = tasks.find((row) => row.id === taskId);
      if (!task) return;
      // No-op on same day — avoids a pointless round-trip and
      // matches the "drag a few pixels onto the source cell" misfire
      // the browser sometimes triggers.
      if (task.scheduled_date === newDayKey) return;
      try {
        await invoke<Task>('update_task', {
          task: {
            ...task,
            scheduled_date: newDayKey,
            // Preserve `scheduled_time` — moving the day shouldn't
            // wipe the user's chosen minute. The carry-over PlanTask
            // flow clears time because the user is explicitly
            // re-planning; a DnD reschedule is a smaller adjustment
            // and the time-of-day usually still makes sense.
            updated_at: new Date().toISOString(),
          },
        });
        announce(
          t('views.week.taskRescheduled', {
            title: task.title,
            date: fmt.format(
              new Date(`${newDayKey}T00:00:00`),
              'PPP',
            ),
          }),
        );
        invalidateData();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('reschedule update_task failed', err);
      }
    },
    [tasks, announce, t, fmt, invalidateData],
  );

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
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t, fmt, invalidateData],
  );
  const handleEventDayDrop = useCallback(
    (ev: CalendarEvent, dayKey: string) => {
      if (isExpandedOccurrence(ev) || ev.recurrence?.rrule) {
        setPendingEventDrop({ event: ev, dayKey });
        return;
      }
      void performEventDrop(ev, dayKey, 'series');
    },
    [performEventDrop],
  );

  const performDelete = useCallback(
    async (ev: CalendarEvent, scope: 'occurrence' | 'series') => {
      try {
        if (scope === 'occurrence' && ev.id.includes('@')) {
          // Mark just this date with an EXDATE on the master so the
          // expansion engine skips it. The master row stays alive
          // and every other occurrence keeps appearing.
          const [seriesId, occIso] = ev.id.split('@');
          await addEventExdate(seriesId, occIso, ev.calendar_id);
          announce(
            t('dialogs.event.occurrenceDeleted', { title: ev.title }),
          );
        } else {
          // Strip the synthetic occurrence suffix — series deletes
          // always target the master row.
          const id = ev.id.includes('@') ? ev.id.split('@')[0] : ev.id;
          await deleteEventById(id, ev.calendar_id);
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
    if (ev.id.includes('@') || ev.recurrence) {
      setScopeTarget(ev);
    } else {
      setConfirmTarget(ev);
    }
  }, []);

  // Deferred indicator — see DayView for the rationale.
  const showLoading = useDeferredLoading(loading);
  useEffect(() => {
    if (showLoading) announce(t('views.loading'));
  }, [showLoading, announce, t]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Tab / Shift+Tab walk through *all* events of the visible week
      // chronologically, crossing day boundaries — the hook moves the
      // anchor and announces the new day when it does.
      if (e.key === 'Tab' && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const consumed = handleTab(e.shiftKey);
        if (consumed) e.preventDefault();
        return;
      }
      if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      }

      // Item-level shortcuts when a chip inside the cell is focused.
      //
      // Events: Enter / Space → open editor; Delete → confirm delete.
      // Tasks:  Enter        → open editor;
      //         Space        → toggle done (matches TaskView's
      //                        long-standing Space-to-check
      //                        convention; the chip's ☐/☑ marker is
      //                        now a real checkbox);
      //         Delete is intentionally NOT bound — task deletion
      //         lives in the dialog so the user confirms it in the
      //         same surface they edit from.
      // Shift+F10 / ContextMenu key opens the chip context menu —
      // matches the platform convention every desktop has used
      // since the 90s.
      if (eventIndex !== null) {
        const item = focusedDayItems[eventIndex];
        if (e.key === 'Escape') {
          e.preventDefault();
          clearEventIndex();
          return;
        }
        if (e.key === 'Enter') {
          e.preventDefault();
          if (item?.kind === 'event') openEventDialog(item.event);
          else if (item?.kind === 'task') openTaskDialog(item.task);
          return;
        }
        if (e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          if (item?.kind === 'event') openEventDialog(item.event);
          else if (item?.kind === 'task') void toggleTaskStatus(item.task);
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
            else void openTaskMenu(item.task, pos);
          }
          return;
        }
        if (e.key === 'Delete' || e.key === 'Backspace') {
          e.preventDefault();
          if (item?.kind === 'event') requestDelete(item.event);
          return;
        }
        // Arrow keys fall through to day navigation below, which will
        // also reset eventIndex via the focusIndex effect.
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
          // Pressing Enter on an empty cell opens the create-event
          // dialog pre-seeded with that day so the form reflects what
          // the user is looking at.
          e.preventDefault();
          const focusedDay = days[focusIndex];
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
      weekStart,
      setAnchor,
      goPrev,
      goNext,
      days,
      focusIndex,
      eventIndex,
      eventsByDay,
      focusedDayItems,
      openEventDialog,
      openTaskDialog,
      handleTab,
      clearEventIndex,
      requestDelete,
      toggleTaskStatus,
      eventOptionId,
      openEventMenu,
      openTaskMenu,
    ],
  );

  const today = useMemo(() => new Date(), []);
  // KW stays ISO-8601 regardless of the visual week start: derive it from a
  // stable mid-week day (Thursday for a Monday start — unchanged default) so
  // a Sunday/Saturday start doesn't slip the number to the adjacent ISO week.
  const isoWeek = fmt.isoWeek(addDays(weekStart, 3));
  // Wait for the first fetch before focusing so the screen reader
  // announces the real day-event count, not the initial empty one.
  const gridRef = useAutoFocus<HTMLDivElement>(!loading);

  return (
    <section className="view view--week" aria-label={t('views.week.title')}>
      <header className="view__header">
        <h2>
          {t('views.week.kw', { week: isoWeek })} ·{' '}
          {fmt.format(weekStart, 'PPP')} – {fmt.format(days[6], 'PPP')}
        </h2>
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
          aria-label={t('views.week.gridLabel')}
          tabIndex={0}
          aria-activedescendant={
            eventIndex !== null
              ? eventOptionId(focusIndex, eventIndex)
              : cellId(focusIndex)
          }
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

          {allDayBars.length > 0 && (
            <div
              className="week-grid__lane"
              aria-hidden="true"
              style={
                { '--lane-rows': laneRows } as React.CSSProperties
              }
            >
              {allDayBars.map((bar) => {
                const color = resolveEventColor(
                  bar.event,
                  calendarById,
                  labelById,
                );
                const isBarFocused = focusedEvId === bar.event.id;
                const span = multiDayInfo(bar.event, new Date(bar.event.start));
                const style: React.CSSProperties & Record<string, string> = {
                  gridColumn: `${bar.startCol} / ${bar.endCol + 1}`,
                  gridRow: String(bar.lane + 1),
                };
                if (color.hex) style['--event-color'] = color.hex;
                return (
                  <div
                    key={bar.event.id}
                    className={
                      'week-allday-bar' +
                      (isBarFocused ? ' week-allday-bar--focused' : '') +
                      (bar.continuesBefore
                        ? ' week-allday-bar--continues-before'
                        : '') +
                      (bar.continuesAfter
                        ? ' week-allday-bar--continues-after'
                        : '')
                    }
                    style={style}
                    // Sighted-only affordance: clicking the bar opens
                    // the event editor; dragging it moves the event to
                    // another day / calendar (the lane is the all-day
                    // event's only VISIBLE representation — the per-day
                    // chips below are clipped a11y anchors). SR users
                    // reach the same actions via the chip + dialogs.
                    draggable
                    onDragStart={(dev) => {
                      setEventDrag(dev.dataTransfer, bar.event);
                    }}
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      openEventDialog(bar.event);
                    }}
                    title={
                      span
                        ? `${bar.event.title} (${span.dayIndex}/${span.totalDays})`
                        : bar.event.title
                    }
                  >
                    {bar.continuesBefore && (
                      <span className="week-allday-bar__chevron" aria-hidden="true">
                        ‹
                      </span>
                    )}
                    <span className="week-allday-bar__title">
                      {bar.event.title}
                    </span>
                    {bar.continuesAfter && (
                      <span className="week-allday-bar__chevron" aria-hidden="true">
                        ›
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          )}

          <div role="row" className="week-grid__body">
            {days.map((day, i) => {
              const dayKey = keyOf(day);
              const merged = dayItemsByDay.get(dayKey);
              const timedItems = merged?.timed ?? [];
              const untimedTasks = merged?.untimed ?? [];
              // The timedItems array is the same one the Tab-navigation
              // buckets see, so `itemIdx` below matches the hook's
              // `eventIndex`. That keeps `aria-activedescendant`,
              // visual focus, and Enter dispatch in sync for both
              // events and timed tasks.
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
                    // Events + tasks (timed + untimed) — every chip in the cell.
                    count: timedItems.length + untimedTasks.length,
                  })}
                  className={
                    'week-grid__cell' +
                    (focused ? ' week-grid__cell--focused' : '') +
                    (isSameDay(day, today) ? ' week-grid__cell--today' : '') +
                    (dragOverDayKey === dayKey
                      ? ' week-grid__cell--drag-over'
                      : '')
                  }
                  onClick={() => setAnchor(day)}
                  onDoubleClick={(e) => {
                    // Double-click on an empty part of the day opens a new event
                    // anchored to it. Skip clicks on a chip (events/tasks are
                    // draggable and have their own double-click → editor).
                    // Keyboard equivalent: Enter on the focused day.
                    if (
                      (e.target as HTMLElement).closest('[draggable="true"]')
                    ) {
                      return;
                    }
                    openEventDialog(null, { defaultDate: keyOf(day) });
                  }}
                  onDragOver={(e) => {
                    // Drop-target gate: react when an Aperio task OR event
                    // drag is in flight. The payload values aren't readable
                    // during dragover, but the type LIST is — and both the
                    // week's own chips and the backlog rail tag the drag
                    // with the task MIME type, so this also accepts backlog
                    // drops (which never set `draggingTaskId`).
                    const types = e.dataTransfer.types;
                    if (
                      !types.includes(TASK_DND_TYPE) &&
                      !types.includes(EVENT_DND_TYPE)
                    ) {
                      return;
                    }
                    e.preventDefault();
                    e.dataTransfer.dropEffect = 'move';
                    if (dragOverDayKey !== dayKey) {
                      setDragOverDayKey(dayKey);
                    }
                  }}
                  onDragLeave={(e) => {
                    // Don't flicker the highlight off when the pointer
                    // moves between the cell's own descendants (chips,
                    // events). Only clear when the pointer actually
                    // leaves the cell box.
                    if (
                      e.relatedTarget instanceof Node &&
                      e.currentTarget.contains(e.relatedTarget)
                    ) {
                      return;
                    }
                    setDragOverDayKey((prev) =>
                      prev === dayKey ? null : prev,
                    );
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    setDragOverDayKey(null);
                    const taskId =
                      e.dataTransfer.getData('text/aperio-task') ||
                      draggingTaskId;
                    if (taskId) {
                      void rescheduleTaskByDrop(taskId, dayKey);
                      return;
                    }
                    const dropped = readEventDrag(e.dataTransfer);
                    if (dropped) handleEventDayDrop(dropped, dayKey);
                  }}
                >
                  <ul role="list" className="week-grid__events">
                    {timedItems.map((item, itemIdx) => {
                      const isFocusedItem =
                        focused && eventIndex === itemIdx;
                      if (item.kind === 'task') {
                        const task = item.task;
                        // Pull the effective time-of-day for this row
                        // on this specific day — could come from either
                        // scheduled_time (the planned slot) or
                        // deadline_time (when this is the deadline day
                        // and only a deadline is set). The same helper
                        // backs taskTimeOnDay-based sorting upstream,
                        // so chips line up consistently.
                        const timeOnDay = taskTimeOnDay(task, dayKey);
                        const time = timeOnDay
                          ? fmt.format(
                              new Date(`${dayKey}T${timeOnDay}`),
                              'p',
                            )
                          : '';
                        const color = resolveTaskColor(
                          task,
                          taskListById,
                          labelById,
                          sectionColorById,
                        );
                        const priorityGlyph = priorityMarker(task.priority);
                        // All four TaskStatus values need their own
                        // glyph + class. The legacy `=== 'completed'`
                        // shortcut would render in_progress and
                        // cancelled identically to open. Cancelled is
                        // filtered out of calendar surfaces already,
                        // but in_progress passes through and would
                        // otherwise look indistinguishable from open.
                        return (
                          <li
                            key={`task-${task.id}`}
                            role="listitem"
                            className="week-grid__task-item"
                          >
                            <span
                              id={eventOptionId(i, itemIdx)}
                              className={
                                'week-task week-task--timed' +
                                (isFocusedItem ? ' week-task--focused' : '') +
                                (draggingTaskId === task.id
                                  ? ' week-task--dragging'
                                  : '') +
                                ` week-task--${task.status.replace('_', '-')}`
                              }
                              // The aria-label carries the status as a
                              // word suffix so SR users hear it on focus
                              // — the visible marker is only there for
                              // sighted users. After Space the toggle
                              // hook also fires a live-region
                              // announcement, so confirmation lands
                              // either way.
                              aria-label={taskChipAriaLabel(
                                t,
                                task,
                                time,
                                tasks,
                              )}
                              aria-selected={isFocusedItem}
                              style={
                                color.hex
                                  ? ({
                                      '--event-color': color.hex,
                                    } as React.CSSProperties)
                                  : undefined
                              }
                              draggable
                              onDragStart={(ev) => {
                                setTaskDrag(
                                  ev.dataTransfer,
                                  task,
                                  tasks.filter((c) => c.parent_id === task.id),
                                );
                                setDraggingTaskId(task.id);
                              }}
                              onDragEnd={() => {
                                setDraggingTaskId(null);
                                setDragOverDayKey(null);
                              }}
                              // Mouse: single click only focuses the day (the
                              // click bubbles to the cell); double click opens
                              // the editor. The marker (below) stops the bubble
                              // so toggling the checkbox doesn't move the anchor.
                              onDoubleClick={(e) => {
                                e.stopPropagation();
                                openTaskDialog(task);
                              }}
                              onContextMenu={(ev) => {
                                ev.preventDefault();
                                ev.stopPropagation();
                                void openTaskMenu(task);
                              }}
                            >
                              <span className="week-task__time">{time}</span>
                              <span className="week-task__body">
                                <span
                                  className="week-task__check"
                                  aria-hidden="true"
                                  onClick={(ev) => {
                                    ev.stopPropagation();
                                    void toggleTaskStatus(task);
                                  }}
                                >
                                  {statusMarker(task.status)}
                                </span>
                                <span className="week-task__title">
                                  {task.parent_id ? '↳ ' : ''}
                                  {task.title}
                                </span>
                                {priorityGlyph && (
                                  <span
                                    className="week-task__priority"
                                    aria-hidden="true"
                                  >
                                    {priorityGlyph}
                                  </span>
                                )}
                              </span>
                            </span>
                          </li>
                        );
                      }
                      const ev = item.event;
                      const cal = calendarById.get(ev.calendar_id);
                      const color = resolveEventColor(ev, calendarById, labelById);
                      const time = ev.all_day
                        ? t('views.allDay')
                        : `${fmt.format(new Date(ev.start), 'p')}`;
                      const span = multiDayInfo(ev, day);
                      // Color label is purely visual — it's a visible
                      // accent strip on the chip, not extra information
                      // an SR user needs spoken. The calendar / list
                      // affiliation stays in the label.
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
                      // All-day events are visualised by the lane above;
                      // their per-day chip stays in the listbox as the
                      // aria-activedescendant target but is clipped out
                      // of the visual flow so the cell only shows timed
                      // events. The bar's focused state is driven from
                      // here via `focusedEvId`.
                      return (
                        <li key={ev.id} role="listitem">
                          <span
                            id={eventOptionId(i, itemIdx)}
                            className={
                              'week-event' +
                              (isFocusedItem ? ' week-event--focused' : '') +
                              (span ? ' week-event--multiday' : '') +
                              (ev.all_day ? ' week-event--in-lane' : '')
                            }
                            aria-label={aria}
                            aria-selected={isFocusedItem}
                            draggable
                            onDragStart={(dev) => {
                              // Drag onto a sidebar calendar row to move the
                              // event there (mouse affordance; the keyboard/SR
                              // path is the Move/Copy dialog).
                              setEventDrag(dev.dataTransfer, ev);
                            }}
                            onDoubleClick={(dcev) => {
                              // Open the editor, mirroring the task chips
                              // (the keyboard path is Enter on the focused
                              // item). Single click stays free for the cell's
                              // day-anchor selection.
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
                            <span className="week-event__time">{time}</span>
                            <span className="week-event__title">{ev.title}</span>
                            {span && (
                              <span className="week-event__span">
                                {t('views.multiDayCompact', {
                                  day: span.dayIndex,
                                  total: span.totalDays,
                                })}
                              </span>
                            )}
                          </span>
                        </li>
                      );
                    })}
                  </ul>
                  {/* §9.4: untimed tasks on this day. Tasks that carry
                      a real deadline_time are hoisted into the timed
                      lane above (sorted between events by time); only
                      scheduled-only tasks and By-window intermediate
                      days end up here. They join the grid's
                      aria-activedescendant nav too — their bucket index
                      continues after the timed lane (`optionIdBase`), so
                      Tab walks them like any other chip. */}
                  <WeekDayTasks
                    tasks={untimedTasks}
                    dayKey={dayKey}
                    allTasks={tasks}
                    cellIndex={i}
                    optionIdBase={timedItems.length}
                    focusedIndex={focused ? eventIndex : null}
                    eventOptionId={eventOptionId}
                    onOpen={(task) => openTaskDialog(task)}
                    onToggle={(task) => {
                      void toggleTaskStatus(task);
                    }}
                    onContextMenu={(task) => {
                      void openTaskMenu(task);
                    }}
                    taskListById={taskListById}
                    labelById={labelById}
                    draggingTaskId={draggingTaskId}
                    onDragStart={(task, ev) => {
                      setTaskDrag(
                        ev.dataTransfer,
                        task,
                        tasks.filter((c) => c.parent_id === task.id),
                      );
                      setDraggingTaskId(task.id);
                    }}
                    onDragEnd={() => {
                      setDraggingTaskId(null);
                      setDragOverDayKey(null);
                    }}
                  />
                </div>
              );
            })}
          </div>
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
        onOccurrence={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'occurrence');
        }}
        onSeries={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'series');
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
    // Multi-day all-day events get bucketed into every visible day they
    // cover — otherwise the user would see day 1 of a vacation and
    // nothing on days 2..N (DESIGN tradeoff: visibility beats compactness,
    // a future iteration may replace the per-day chips with one
    // continuous bar in a dedicated all-day lane). Timed events stay
    // anchored to their start day; cross-midnight meetings are out of
    // scope here.
    daysCoveredKeys(ev).forEach((k) => {
      const bucket = map.get(k);
      if (bucket) bucket.push(ev);
    });
  });
  return map;
}

// ────────────────────────────────────────────────────────────────────────
// Per-day task chips (§9.4)
// ────────────────────────────────────────────────────────────────────────

/**
 * Small list of tasks visible on one day cell. Buttons rather than
 * spans so they're activatable via Enter/Space and Tab-reachable for
 * keyboard / SR users without the cell-internal aria-activedescendant
 * choreography the event chips need.
 *
 * Activating a chip opens the TaskDialog (edit mode) — same flow as
 * Enter on a row in the dedicated Aufgaben view. Status-toggle and
 * "Im Backlog ablegen" stay in TaskView's keyboard surface; the
 * calendar chips are display + drill-into-detail only.
 */
function WeekDayTasks({
  tasks,
  dayKey,
  allTasks,
  cellIndex,
  optionIdBase,
  focusedIndex,
  eventOptionId,
  onOpen,
  onToggle,
  onContextMenu,
  taskListById,
  labelById,
  draggingTaskId,
  onDragStart,
  onDragEnd,
}: {
  tasks: Task[];
  /** ISO day key of this column — lets a chip tell whether it's here
   *  because it's DUE today (deadline marker) vs scheduled today. */
  dayKey: string;
  /** All tasks in the store — used to resolve subtask progress
   *  for the parents that show up in this day's chip list. */
  allTasks: Task[];
  /** This day's index in the week, for the activedescendant option id. */
  cellIndex: number;
  /** Bucket index of the first untimed task: the count of timed items in
   *  this cell, so untimed chips continue the grid nav after the lane. */
  optionIdBase: number;
  /** The grid's focused item index when THIS cell is the focused day,
   *  else null — drives the chip's focus ring + `aria-selected`. */
  focusedIndex: number | null;
  /** Builds the `aria-activedescendant` option id, shared with the grid so
   *  the keyboard nav and these chips agree on which one is focused. */
  eventOptionId: (cellIdx: number, itemIdx: number) => string;
  onOpen: (task: Task) => void;
  onToggle: (task: Task) => void;
  onContextMenu: (task: Task) => void;
  taskListById: Map<string, import('../../api/types').TaskList>;
  labelById: Map<string, import('../../api/types').ColorLabel>;
  /** Currently-dragged task id (if any). Drives the dimming class
   *  on the source chip so the user sees which row is being
   *  rescheduled. */
  draggingTaskId: string | null;
  /** Drag start handler — receives the task plus the native event so
   *  the parent can set `dataTransfer.setData` and update its
   *  `draggingTaskId` state. */
  onDragStart: (task: Task, ev: React.DragEvent<HTMLElement>) => void;
  /** Drag end handler — fires on both successful drop and cancel
   *  (Esc / drop outside any target). The parent uses it to clear
   *  the dragging state. */
  onDragEnd: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { sectionColorById } = useCalendarStore();
  if (tasks.length === 0) return null;
  return (
    <ul
      className="week-grid__tasks"
      aria-label={t('views.week.tasksOnDay', { count: tasks.length })}
    >
      {tasks.map((task, idx) => {
        // `isBy` = the task is here as a pure deadline marker (a deadline-only
        // task on its due day) — it keeps the hard-edge `--by` ring. A task
        // with a scheduled day now shows ONLY on that day and announces its
        // deadline there, so the "fällig bis …" label is used whenever the
        // task carries a deadline.
        const isBy = isDeadlineChip(task, dayKey);
        const hasDeadline = task.deadline_date != null;
        const labelKey = hasDeadline
          ? 'views.week.taskChipBy'
          : 'views.week.taskChip';
        // Visible deadline badge on the SCHEDULED chip (the deadline-only
        // marker already sits on its own day, so it needs none).
        const deadlineBadge =
          !isBy && task.deadline_date
            ? fmt.format(new Date(`${task.deadline_date}T00:00:00`), 'P')
            : '';
        const color = resolveTaskColor(
          task,
          taskListById,
          labelById,
          sectionColorById,
        );
        const state = t(statusI18nKey(task.status));
        const priorityGlyph = priorityMarker(task.priority);
        // Bucket index of this untimed chip (after the timed lane). The
        // grid's keyboard nav focuses it via aria-activedescendant, so it's
        // a focus *target* (a span) like the timed chips — not a separate
        // tabbable button. Keyboard (Enter / Space / menu) is dispatched by
        // the grid's handleKeyDown; mouse click + drag stay on the chip.
        const navIndex = optionIdBase + idx;
        const isFocused = focusedIndex === navIndex;
        return (
          <li
            key={task.id}
            role="listitem"
            className="week-grid__task-item"
          >
            <span
              id={eventOptionId(cellIndex, navIndex)}
              className={
                'week-task' +
                (isFocused ? ' week-task--focused' : '') +
                ` week-task--${task.status.replace('_', '-')}` +
                (isBy ? ' week-task--by' : '') +
                (draggingTaskId === task.id
                  ? ' week-task--dragging'
                  : '')
              }
              aria-selected={isFocused}
              draggable
              onDragStart={(ev) => onDragStart(task, ev)}
              onDragEnd={onDragEnd}
              onDoubleClick={(e) => {
                e.stopPropagation();
                onOpen(task);
              }}
              onContextMenu={(ev) => {
                ev.preventDefault();
                ev.stopPropagation();
                onContextMenu(task);
              }}
              style={
                color.hex
                  ? ({ '--event-color': color.hex } as React.CSSProperties)
                  : undefined
              }
              aria-label={t(labelKey, {
                title: task.title,
                deadline: task.deadline_date
                  ? fmt.format(
                      new Date(`${task.deadline_date}T00:00:00`),
                      'PPP',
                    )
                  : '',
                state,
                priority: prioritySuffix(t, task.priority),
                progress: subtaskProgressSuffix(t, task.id, allTasks),
                assignee: assigneeSuffix(t, task.assignees),
              }) + subtaskParentSuffix(t, task, allTasks)}
            >
              <span className="week-task__body">
                <span
                  className="week-task__check"
                  aria-hidden="true"
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onToggle(task);
                  }}
                >
                  {statusMarker(task.status)}
                </span>
                <span className="week-task__title">
                  {task.parent_id ? '↳ ' : ''}
                  {task.title}
                </span>
                {priorityGlyph && (
                  <span className="week-task__priority" aria-hidden="true">
                    {priorityGlyph}
                  </span>
                )}
                {deadlineBadge && (
                  <span className="week-task__deadline" aria-hidden="true">
                    {t('views.week.taskChipDeadlineBadge', {
                      deadline: deadlineBadge,
                    })}
                  </span>
                )}
              </span>
            </span>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * Build the SR label for the timed task chip. Centralised so the chip
 * inside the listbox and the untimed variant in WeekDayTasks format
 * identically — including the status word suffix that turns the
 * visible marker glyph into something AT can read out, plus the
 * "{{done}} of {{total}} subtasks done" segment when relevant.
 */
function taskChipAriaLabel(
  t: (key: string, values?: Record<string, unknown>) => string,
  task: Task,
  time: string,
  allTasks: Task[],
): string {
  const state = t(statusI18nKey(task.status));
  return t('views.week.taskChipTimed', {
    title: task.title,
    time,
    state,
    priority: prioritySuffix(t, task.priority),
    progress: subtaskProgressSuffix(t, task.id, allTasks),
    assignee: assigneeSuffix(t, task.assignees),
  }) + subtaskParentSuffix(t, task, allTasks);
}
