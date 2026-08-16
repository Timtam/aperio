import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
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
  eventDayTimes,
  multiDayInfo,
} from '../../intl/multiDay';
import { DayCheckInButton } from '../DayCheckInButton';
import { useDayLogSummaries } from '../../state/useDayLogSummaries';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { canSetTaskTime } from '../../state/taskMoves';
import {
  EVENT_DND_TYPE,
  moveEventToSlot,
  readEventDrag,
  readTaskDrag,
  scheduleTaskAtTime,
  scheduleTaskOnDay,
  setEventDrag,
  setTaskDrag,
  TASK_DND_TYPE,
  type MoveCopyScope,
} from '../../state/moveActions';
import {
  isSeriesOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../../intl/recurrence';
import { MoveEventScopeDialog } from '../MoveEventScopeDialog';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useEventGroups } from '../../state/useEventGroups';
import { useTaskListShowCompleted } from '../../state/useTaskListShowCompleted';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange } from '../../state/viewMath';
import {
  expandScheduledRecurringTasks,
  groupTasksByDay,
  isDeadlineChip,
  isRecurringProjection,
  mergeDayItems,
  recurringSeriesTaskId,
  taskEndTimeOnDay,
  taskTimeOnDay,
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
  collapseEventGroups,
  groupBadge,
  dropMinuteInWindow,
  eventBlockFactor,
  eventSpanForDay,
  eventInstanceKey,
  layoutDayColumn,
  MINUTES_PER_DAY,
  minutesFromMidnight,
  type CollapsedRow,
  type PositionedSpan,
  type PriorityScale,
  type TimedSpan,
} from '@aperio/shared';

/** Base block height (rem) a LIST-mode event chip gets at `eventBlockFactor === 1`
 *  (a point / ≤1h event) — one line of text plus a little fill, tuned for a fuller
 *  use of the column's vertical space. The list-mode chip uses a
 *  STRICT height (not min-height) of `factor × this`, with the time + title on one
 *  wrapping line clipped to fit, so the chip height reads DURATION at a glance and
 *  a long title can never inflate a short event past a long one. The shared
 *  `eventBlockFactor` curve is ~linear in hours (a 3h event ≈ 3×, a 4h ≈ 4×,
 *  capped at 6×). rem (not em) so the small chip font doesn't shrink the scale. */
const WEEK_LIST_BLOCK_BASE_REM = 2.25;

/** The slot's CSS `min-height` (1.2em, see `.week-grid__slot`) expressed as a
 *  fraction of the FULL-DAY canvas, so a floored late-night chip can be clamped
 *  to stay on-canvas. 1.2em ≈ 26px of a ~1440px-ish day canvas → ~0.018; the
 *  exact value only needs to match the rendered min-height closely enough that a
 *  zero-duration 23:5x chip doesn't overflow the bottom into the untimed list.
 *  With a narrower visible window the canvas is shorter, so the same absolute
 *  min-height is a LARGER fraction — callers scale this up via `slotStyle`'s
 *  `minFraction` arg. Matches DayView's MIN_SLOT_FRACTION. */
const MIN_SLOT_FRACTION = 0.018;

/** How much taller than the base 1.2em slot floor the effort classes render
 *  a TASK chip (`.week-task--effort-medium` 1.9em / `--effort-large` 2.6em in
 *  styles.css). The top-clamp must reserve the REAL rendered height, or a
 *  floored chip near the window end paints past the canvas bottom over the
 *  outside band / untimed lane — the desktop twin of mobile's
 *  GRID_TASK_EFFORT_PX floor. '' (small, or sizing off) keeps the base floor.
 *  Matches DayView's effortSlotFactor. */
function effortSlotFactor(effortMod: string): number {
  if (effortMod === 'medium') return 1.9 / 1.2;
  if (effortMod === 'large') return 2.6 / 1.2;
  return 1;
}

/** Absolute placement of a timed chip's `<li>` inside the day column's
 *  visible-window hour-grid (positioning is purely visual; DOM order is
 *  unchanged). The TOP is clamped (not the height) so a chip whose effective
 *  height is the CSS min-height floor can't extend below the canvas and overlap
 *  the untimed `.week-grid__tasks` rendered directly under it — mirrors the
 *  mobile slot clamp. `minFraction` is the floored option's min-height as a
 *  fraction of the CURRENT canvas (the window, which may be < 24h) — a wider
 *  fraction on a narrow window keeps the bottom clamp correct. A normal chip
 *  (heightFraction ≥ minFraction ending ≤ window end) is unaffected; only a
 *  floored chip in the last ~30min shifts up a hair. */
function slotStyle(
  p: PositionedSpan,
  minFraction = MIN_SLOT_FRACTION,
): React.CSSProperties {
  const eh = Math.min(1, Math.max(p.heightFraction, minFraction));
  const top = Math.max(0, Math.min(p.topFraction, 1 - eh));
  return {
    position: 'absolute',
    top: `${top * 100}%`,
    height: `${p.heightFraction * 100}%`,
    left: `${(p.columnIndex / p.columnCount) * 100}%`,
    width: `${(1 / p.columnCount) * 100}%`,
  };
}

/** One chip in a cell's "outside the visible hours" band (before/after the
 *  window). Holds just what the decorative band needs; the real a11y is the
 *  clipped chip this duplicates (the `--in-window-clip` chip inside the cell).
 *  Mirrors DayView's OutsideBandEntry. */
interface OutsideBandEntry {
  key: string;
  title: string;
  /** Localised start time, e.g. "06:00" — so the bar reads "06:00 Title". */
  time: string;
  colorHex?: string;
  onOpen: () => void;
}

/** Decorative band of one cell's events/tasks that fall outside the visible day
 *  window — rendered at the TOP (before) / BOTTOM (after) of that day's content.
 *  `aria-hidden`: each entry is also a clipped, navigable chip inside the cell
 *  (that's where the a11y lives), so the bars are `tabIndex={-1}` and exist only
 *  for sighted users. Returns null when empty. Mirrors DayView's OutsideBand,
 *  scoped per cell. */
function WeekOutsideBand({
  entries,
  edge,
  label,
}: {
  entries: OutsideBandEntry[];
  edge: 'before' | 'after';
  label: string;
}) {
  if (entries.length === 0) return null;
  return (
    <div
      className={`week-grid__outside week-grid__outside--${edge}`}
      aria-hidden="true"
    >
      <span className="week-grid__outside-label">{label}</span>
      {entries.map((e) => (
        <button
          key={e.key}
          type="button"
          tabIndex={-1}
          className="week-grid__outside-bar"
          style={
            e.colorHex
              ? ({ '--event-color': e.colorHex } as React.CSSProperties)
              : undefined
          }
          onClick={e.onOpen}
        >
          {e.time && <span className="week-grid__outside-time">{e.time}</span>}
          <span className="week-grid__outside-title">{e.title}</span>
        </button>
      ))}
    </div>
  );
}

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
  const { openEventDialog, openTaskDialog, openCreateChooser, invalidateData } =
    useDialogState();

  const range = useMemo(
    () => visibleRange('week', anchor, weekStartsOn),
    [anchor, weekStartsOn],
  );
  const { events: allEvents, calendarById, loading } = useEvents(range);
  const { tasks, taskListById } = useTasks();
  const {
    visualEffortSizing,
    dayViewMode,
    dayStartMin,
    dayEndMin,
    priorityScale,
  } = useTaskCascadeEnabled();
  // Compact-list layout vs the proportional hour-grid. In list mode the
  // per-day <ul> is normal vertical flow (no positioned 24h canvas), the
  // ruler + all-day lane are not rendered, and each chip carries an inline
  // min-height (events by duration, tasks by effort) instead of a slot. The
  // a11y model — roles, ids, aria-activedescendant, keyboard, labels — is
  // byte-for-byte identical to grid mode.
  const listMode = dayViewMode === 'list';
  // Visible day window (synced pref) — each day column's hour-grid spans
  // [dayStartMin, dayEndMin] instead of a fixed 0–24, so the canvas height +
  // ruler shrink to the window and timed chips position relative to it (see
  // layoutDayColumn below). Default 0/1440 reproduces the full day exactly.
  // Mirrors DayView, the proven reference (commits a8c7d65 + c41b9a2 + fa5aae8).
  const windowMin = Math.max(1, dayEndMin - dayStartMin);
  const dayHours = windowMin / 60;
  const gridLineFrac = ((60 - (dayStartMin % 60)) % 60) / 60;
  // Bake the windowed canvas height straight into an inline style (the number
  // interpolated into the string) rather than driving it through a React-inline
  // CSS custom property: a custom prop consumed inside calc() proved unreliable
  // here (that was the whole bug a8c7d65 fixed — the grid stayed 24h). `--hour-px`
  // itself is CSS-defined, so it resolves fine. List mode has no canvas (the
  // --flow rules drive height), so this is gated on !listMode at every use site.
  const gridHeight = `calc(${dayHours} * var(--hour-px, 2.5rem))`;
  // Shift the hourly gridline gradient so the lines land on whole hours even when
  // the window starts on a half-hour (applied inline on each cell's canvas).
  const gridLineOffset = `calc(${gridLineFrac} * var(--hour-px, 2.5rem))`;
  // The slot min-height (MIN_SLOT_FRACTION of the FULL day) as a fraction of the
  // current (possibly narrower) window, so a floored point keeps its on-canvas
  // clamp at any window size. Capped so a very narrow window stays sane.
  const slotMinFraction = Math.min(
    0.5,
    (MIN_SLOT_FRACTION * MINUTES_PER_DAY) / windowMin,
  );
  // Whole-hour ruler ticks inside the window, INCLUDING the window-end hour so
  // the chosen end is labelled (e.g. 7…23 for a 7–23 window) — but never 24:00,
  // the degenerate full-day end (so the default stays 0…23 as before).
  const rulerHours = useMemo(() => {
    const out: number[] = [];
    for (
      let h = Math.ceil(dayStartMin / 60);
      h * 60 <= dayEndMin && h * 60 < MINUTES_PER_DAY;
      h += 1
    ) {
      out.push(h);
    }
    return out;
  }, [dayStartMin, dayEndMin]);
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
  const { colorLabels, sectionColorById, sectionsByList, loadSections, taskLists } =
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

  // What each day in the window was marked with. ONE range read for the whole
  // window, hung on the cells' own accessible names — the overview costs no
  // extra focus stop, which is the entire point of putting it there.
  const dayMarkerKeys = useMemo(() => days.map((d) => keyOf(d)), [days]);
  const { symbolsFor, spokenFor } = useDayLogSummaries(
    dayMarkerKeys,
    t('dialogs.dayLog.summaryLead'),
  );


  // The range as well as the events: with both in hand the hook can also
  // repair members whose provider id changed, and group a videoconference
  // meeting with the appointment it belongs to (DESIGN-event-groups.md). It
  // hands back the rows to show — without the meetings whose appointment is
  // in view and not grouped with them.
  const { groups, events } = useEventGroups(allEvents, range);
  /**
   * The week's events per day, with each group folded into ONE chip.
   *
   * Folding here rather than at render time is what makes the rest of the
   * week fall into place for free: `mergeDayItems`, the Tab buckets and
   * `layoutDayColumn` all read this map, so a folded-away copy stops taking
   * a column in the hour grid as well as a line in the reading order.
   *
   * Per day, as `collapseEventGroups` documents — across the week a recurring
   * appointment's own days would look exactly like copies that disagree.
   */
  const { eventsByDay, groupRows } = useMemo(() => {
    const raw = groupEventsByDay(events, days);
    const folded = new Map<string, CalendarEvent[]>();
    const rows = new Map<string, CollapsedRow<CalendarEvent>>();
    for (const [dayKey, dayEvents] of raw) {
      const collapsed = collapseEventGroups(dayEvents, groups, seriesIdOf);
      folded.set(
        dayKey,
        collapsed.map((row) => row.event),
      );
      for (const row of collapsed) {
        if (row.group) rows.set(`${eventInstanceKey(row.event)}@${dayKey}`, row);
      }
    }
    return { eventsByDay: folded, groupRows: rows };
  }, [events, days, groups]);

  /**
   * The events the all-day LANE may draw.
   *
   * The lane is where an all-day event actually appears — its chip inside the
   * cell is clipped to a 1x1 rect — so drawing it from the unfolded list left
   * one bar per copy above a listbox that had folded them into one row: the
   * screen contradicted what it announced, and each bar was separately
   * draggable, so moving the second one silently pulled the group apart.
   *
   * Filtered by what survived folding on ANY day rather than rebuilt per day,
   * so a multi-day bar stays one bar instead of fragmenting where different
   * days picked different representatives.
   */
  const laneEvents = useMemo(() => {
    const kept = new Set<string>();
    for (const dayEvents of eventsByDay.values()) {
      for (const ev of dayEvents) kept.add(eventInstanceKey(ev));
    }
    return events.filter((ev) => kept.has(eventInstanceKey(ev)));
  }, [events, eventsByDay]);

  // Bucket tasks per visible day (§9.4). A task lands on a day when it
  // is scheduled for that day OR due (deadline) that day — the deadline
  // shows as a point marker on its deadline day, not a span across every
  // day until then (see `filterTasksOnDay`). A task scheduled on one day
  // and due on another therefore appears on both; same-day collapses to
  // a single chip.
  const tasksByDay = useMemo(() => {
    const dayKeys = days.map((d) => keyOf(d));
    // Expand recurring SCHEDULED tasks into one occurrence per planned day
    // across the visible week — so a task recurring every day/week shows on
    // EVERY due day (like a recurring event), not only its single current
    // scheduled_date. The occurrence on the task's own date is the real,
    // interactive task; the others are read-only projections
    // (isRecurringProjection) that open the series and offer no
    // complete/reschedule/delete. Non-recurring / from-completion / backlog
    // tasks pass through untouched.
    let fromKey = dayKeys[0] ?? '';
    let toKey = dayKeys[0] ?? '';
    for (const k of dayKeys) {
      if (k < fromKey) fromKey = k;
      if (k > toKey) toKey = k;
    }
    const expanded =
      dayKeys.length > 0
        ? expandScheduledRecurringTasks(tasks, fromKey, toKey)
        : tasks;
    return groupTasksByDay(
      expanded,
      dayKeys,
      shouldShowCompletedForList,
      meFor,
      priorityScale,
    );
  }, [tasks, days, shouldShowCompletedForList, meFor, priorityScale]);

  // Resolve a (possibly projected) task back to its real series task so opening
  // a projection opens the underlying task, not a non-existent occurrence id. A
  // no-op for a real task.
  const seriesTaskOf = useCallback(
    (task: Task): Task => {
      if (!isRecurringProjection(task)) return task;
      const id = recurringSeriesTaskId(task.id);
      return tasks.find((x) => x.id === id) ?? task;
    },
    [tasks],
  );

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

  // Hour-grid placement per day: each non-all-day timed item gets an absolute
  // slot (top/height by start+duration, side-by-side on overlap) WITHIN the
  // visible window [dayStartMin, dayEndMin]. All-day chips stay clipped in the
  // lane (no slot). Items entirely outside the window get a 'before'/'after'
  // placement (clipped + shown in the cell's outside band). Keyed by the day key
  // → (item index → PositionedSpan); DOM order (SR/keyboard nav) is untouched.
  // Hoisted to ONE memo (shared by the render loop + the outside-band builder)
  // so layoutDayColumn runs once per day. Mirrors DayView's single slotByIdx.
  const slotByDay = useMemo(() => {
    const out = new Map<string, Map<number, PositionedSpan>>();
    for (const day of days) {
      const dayKey = keyOf(day);
      const timedItems = dayItemsByDay.get(dayKey)?.timed ?? [];
      const map = new Map<number, PositionedSpan>();
      const spans: TimedSpan[] = [];
      const slotIdxs: number[] = [];
      timedItems.forEach((item, idx) => {
        let s: TimedSpan | null = null;
        if (item.kind === 'event') {
          // All-day events stay clipped in the lane (no meaningful hour slot).
          if (!item.event.all_day) {
            s = eventSpanForDay(
              new Date(item.event.start),
              new Date(item.event.end),
              day,
            );
          }
        } else {
          // A timed task is a point unless the user planned a block for it,
          // in which case it occupies its hours exactly like an event does —
          // that is what a planned block IS. An unparseable time falls back to
          // midnight so the item ALWAYS gets a slot and never flows static
          // inside the positioned canvas (which would corrupt the grid).
          const m = minutesFromMidnight(taskTimeOnDay(item.task, dayKey) ?? '');
          const end = minutesFromMidnight(
            taskEndTimeOnDay(item.task, dayKey) ?? '',
          );
          const startMin = m ?? 0;
          s = {
            startMin,
            endMin: end != null && end > startMin ? end : startMin,
          };
        }
        if (s) {
          spans.push(s);
          slotIdxs.push(idx);
        }
      });
      const positions = layoutDayColumn(spans, {
        startMin: dayStartMin,
        endMin: dayEndMin,
      });
      slotIdxs.forEach((idx, k) => map.set(idx, positions[k]));
      out.set(dayKey, map);
    }
    return out;
  }, [days, dayItemsByDay, dayStartMin, dayEndMin]);

  // Window-boundary time as a localised "HH:MM" for the outside-band labels.
  // 1440 is the end-of-day sentinel (no real Date), so spell it literally.
  // Mirrors DayView's clockAt; the dayKey is the band's own day so the format
  // is locale-correct (the value itself is the same wall-clock on every day).
  const clockAt = useCallback(
    (min: number, dayKey: string): string => {
      if (min >= MINUTES_PER_DAY) return '24:00';
      const hh = String(Math.floor(min / 60)).padStart(2, '0');
      const mm = String(min % 60).padStart(2, '0');
      return fmt.format(new Date(`${dayKey}T${hh}:${mm}:00`), 'p');
    },
    [fmt],
  );

  // Items ENTIRELY outside the visible window (placement 'before'/'after') per
  // day: their chip is clipped (visually hidden but still a navigable focus
  // target reading the real time off its label — exactly like an all-day chip),
  // and the SIGHTED representation is a compact band at the TOP (before) / BOTTOM
  // (after) of that cell. Built in time order; events AND timed tasks both land
  // here. Mirrors DayView's outsideBands, scoped per cell.
  const outsideBandsByDay = useMemo(() => {
    const out = new Map<
      string,
      { before: OutsideBandEntry[]; after: OutsideBandEntry[] }
    >();
    for (const day of days) {
      const dayKey = keyOf(day);
      const timedItems = dayItemsByDay.get(dayKey)?.timed ?? [];
      const slotByIdx = slotByDay.get(dayKey);
      const before: OutsideBandEntry[] = [];
      const after: OutsideBandEntry[] = [];
      timedItems.forEach((item, idx) => {
        const slot = slotByIdx?.get(idx);
        if (!slot || slot.placement === 'in') return;
        let entry: OutsideBandEntry;
        if (item.kind === 'event') {
          const ev = item.event;
          // Cross-midnight event: its tail on day 2 runs 00:00–…, so show the
          // clamped THIS-DAY start (like the grid chip + aria-label do), not the
          // misleading absolute start.
          const startStr = multiDayInfo(ev, day)
            ? eventDayTimes(fmt, ev, day).startStr
            : fmt.format(new Date(ev.start), 'p');
          entry = {
            key: `ev-${ev.id}`,
            title: ev.title,
            time: startStr,
            colorHex:
              resolveEventColor(ev, calendarById, labelById).hex ?? undefined,
            onOpen: () => openEventDialog(ev),
          };
        } else {
          const task = item.task;
          const t0 = taskTimeOnDay(task, dayKey);
          entry = {
            key: `task-${task.id}`,
            title: task.title,
            time: t0 ? fmt.format(new Date(`${dayKey}T${t0}`), 'p') : '',
            colorHex:
              resolveTaskColor(task, taskListById, labelById, sectionColorById)
                .hex ?? undefined,
            onOpen: () => openTaskDialog(seriesTaskOf(task)),
          };
        }
        (slot.placement === 'before' ? before : after).push(entry);
      });
      out.set(dayKey, { before, after });
    }
    return out;
  }, [
    days,
    dayItemsByDay,
    slotByDay,
    calendarById,
    labelById,
    taskListById,
    sectionColorById,
    fmt,
    openEventDialog,
    openTaskDialog,
    seriesTaskOf,
  ]);

  // Build the all-day lane bars over the week. The lane is the
  // visual half of variant B: a contiguous strip above the day cells
  // where each multi-day all-day event spans the columns it covers.
  // SR users still find the underlying event via the per-day chips
  // inside the cells (those carry the listbox options) — the lane
  // here is `aria-hidden` and exists only for sighted users.
  const allDayBars = useMemo(
    () => buildAllDayBars(laneEvents, days),
    [laneEvents, days],
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
  // Where the current drag of a GRID chip started (cleared on its dragend).
  // The browser can misfire a few-pixel drag out of a double-click; the
  // day-drop below no-ops those via its same-day check, but the canvas
  // time-drop would happily write a new minute — so drops that barely moved
  // from this point are ignored (see dropTaskAtTime).
  const dragOriginRef = useRef<{ x: number; y: number } | null>(null);

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

  // Drag-to-time: a task dropped onto a day's hour-grid CANVAS (not just
  // the cell) gets the drop position's wall-clock time (snapped to 15 min)
  // on top of the day — it turns into a timed chip right where it landed.
  // The plain cell drop below keeps the day-only reschedule for drops on
  // the untimed band / cell padding. Grid mode only (the canvas exists
  // only there).
  const dropTaskAtTime = useCallback(
    (e: React.DragEvent<HTMLElement>, dayKey: string) => {
      const payload = readTaskDrag(e.dataTransfer);
      if (!payload) return; // event drag → bubble on to the cell handler
      e.preventDefault();
      e.stopPropagation();
      setDragOverDayKey(null);
      const origin = dragOriginRef.current;
      if (
        origin &&
        Math.abs(e.clientX - origin.x) < 8 &&
        Math.abs(e.clientY - origin.y) < 8
      ) {
        return; // double-click misfire, not a reposition
      }
      const rect = e.currentTarget.getBoundingClientRect();
      const fraction =
        rect.height > 0 ? (e.clientY - rect.top) / rect.height : 0;
      const minute = dropMinuteInWindow(fraction, {
        startMin: dayStartMin,
        endMin: dayEndMin,
      });
      const clock = `${String(Math.floor(minute / 60)).padStart(2, '0')}:${String(minute % 60).padStart(2, '0')}:00`;
      // Write the CURRENT row, not the dragstart snapshot — a sync refresh
      // can land mid-drag, and re-sending the stale snapshot with a fresh
      // updated_at would silently revert another device's edit (the sibling
      // day-drop resolves from the store for the same reason).
      const current =
        tasks.find((row) => row.id === payload.task.id) ?? payload.task;
      void (async () => {
        try {
          // Google Tasks, Microsoft To Do and Exchange keep whole days. Writing
          // the minute there succeeds and the value is gone by the next refresh,
          // so the drop takes the DAY — which those sources can hold — and says
          // that is what happened rather than announcing a time that will not
          // survive the trip.
          const keepsTime = canSetTaskTime(
            taskLists.find((l) => l.id === current.list_id),
          );
          if (keepsTime) {
            await scheduleTaskAtTime(current, dayKey, minute);
          } else {
            // The time goes with it. `scheduleTaskOnDay` keeps a time-of-day by
            // design — that is right for the week planner's day drop — but here
            // the announcement says the time was not kept, and an announcement
            // is the only feedback a screen-reader user gets. Leaving a stale
            // time behind would send the chip back to an hour nowhere near where
            // it was dropped while claiming otherwise. It is also the only way
            // left to clear a time the editor no longer offers on this list.
            await scheduleTaskOnDay(
              { ...current, scheduled_time: null, scheduled_end_time: null },
              dayKey,
            );
          }
          announce(
            t(keepsTime ? 'views.taskScheduledAtTime' : 'views.taskScheduledDayOnly', {
              title: current.title,
              date: fmt.format(new Date(`${dayKey}T00:00:00`), 'PPP'),
              time: fmt.format(new Date(`${dayKey}T${clock}`), 'p'),
            }),
          );
          invalidateData();
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn('drop scheduleTaskAtTime failed', err);
        }
      })();
    },
    [tasks, taskLists, dayStartMin, dayEndMin, announce, t, fmt, invalidateData],
  );

  // Event chip dropped on a day cell → move it there (time + duration
  // stay). Recurring events first ask for the §7.5 scope ("only this
  // occurrence / whole series") via the pending state + dialog below.
  const [pendingEventDrop, setPendingEventDrop] = useState<{
    event: CalendarEvent;
    dayKey: string;
    /** Carried across the scope question so the answer lands on the time the
     *  user actually dropped on, not on the old one. */
    minute: number | null;
  } | null>(null);
  const performEventDrop = useCallback(
    async (
      ev: CalendarEvent,
      dayKey: string,
      scope: MoveCopyScope,
      /** Minute of day the drop landed on, or null for a day-only move (the
       *  all-day lane, and the list mode that has no hour geometry). */
      minute: number | null = null,
    ) => {
      try {
        const moved = await moveEventToSlot(ev, dayKey, minute, scope);
        if (!moved) return; // nothing changed — nothing to announce
        announce(
          minute === null || ev.all_day
            ? t('views.eventMovedToDay', {
                title: ev.title,
                date: fmt.format(new Date(`${dayKey}T00:00:00`), 'PPP'),
              })
            : t('views.eventMovedToTime', {
                title: ev.title,
                date: fmt.format(new Date(`${dayKey}T00:00:00`), 'PPP'),
                time: clockAt(minute, dayKey),
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
    [announce, t, fmt, invalidateData, clockAt],
  );
  const handleEventDayDrop = useCallback(
    (ev: CalendarEvent, dayKey: string, minute: number | null = null) => {
      if (isSeriesOccurrence(ev) || ev.recurrence?.rrule) {
        setPendingEventDrop({ event: ev, dayKey, minute });
        return;
      }
      void performEventDrop(ev, dayKey, 'series', minute);
    },
    [performEventDrop],
  );

  // Map a drop's horizontal position to the day column under the cursor by
  // testing the hour-grid day cells (which share the lane's column grid, so no
  // gap/padding math). Lets an all-day bar dragged sideways along the lane — or
  // dropped over another bar — still resolve to the day it landed on.
  const dayKeyAtClientX = (clientX: number): string | null => {
    for (let i = 0; i < days.length; i += 1) {
      const cellEl = document.getElementById(cellId(i));
      if (!cellEl) continue;
      const r = cellEl.getBoundingClientRect();
      if (clientX >= r.left && clientX < r.right) return keyOf(days[i]);
    }
    return null;
  };

  const performDelete = useCallback(
    async (
      ev: CalendarEvent,
      scope: 'occurrence' | 'this_and_future' | 'series',
      sendCancellations = false,
    ) => {
      try {
        // `occurrenceIsoOf` / `seriesIdOf` rather than splitting the id on
        // `@`: an id only carries a synthetic occurrence suffix when the
        // expansion engine put one there, and a RECURRENCE-ID override marks
        // itself with `::rid::`. Splitting on `@` mistook the domain part of a
        // perfectly ordinary UID — `abc@aperio`, or whatever iCloud minted —
        // for an occurrence instant, which is what made a single event ask
        // the occurrence-or-series question and then delete a truncated id.
        const occIso = occurrenceIsoOf(ev);
        if (scope === 'occurrence' && occIso) {
          // Mark just this date with an EXDATE on the master so the
          // expansion engine skips it. The master row stays alive
          // and every other occurrence keeps appearing.
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
          // Truncate the series so it ends just before this occurrence.
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
          // Series deletes always target the master row.
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
    // Only an EXPANDED occurrence has a specific instance to delete, so only it
    // gets the occurrence-vs-series choice. A recurring MASTER row (e.g. an
    // unparseable RRULE that couldn't be expanded) has no single occurrence —
    // offering "this occurrence" there would fall through to a full-series
    // delete — so it takes the plain confirm (which deletes the series).
    if (isSeriesOccurrence(ev)) {
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

  // The active item is an all-day event whose per-day chip is clipped into the
  // lane (`--in-lane`) at static y0 (00:00); scrolling THAT into view would yank
  // the 24h region up to midnight every time the user Tabs onto an all-day
  // event. So skip the nudge for all-day items — they're already shown in the
  // always-visible lane bar above the canvas.
  const focusedItemIsAllDay =
    focusedItem?.kind === 'event' && focusedItem.event.all_day;

  // An outside-window item is ALSO clipped at static y0 (placement before/after,
  // no positioned slot), so scrolling it would yank the canvas to the window
  // start just like an all-day chip — skip the nudge for it too.
  const focusedSlotPlacement =
    eventIndex !== null
      ? slotByDay.get(keyOf(days[focusIndex]))?.get(eventIndex)?.placement
      : undefined;
  const focusedItemOutside =
    focusedSlotPlacement === 'before' || focusedSlotPlacement === 'after';

  // The timed grid is a 24h-tall internal scroll region now, and
  // aria-activedescendant (unlike a real DOM .focus()) does NOT auto-scroll the
  // active chip into view — so a chip on a scrolled-away hour would be off-screen
  // for sighted / low-vision keyboard users. Scroll it into view whenever the
  // active timed chip changes.
  useEffect(() => {
    // List mode is a normal vertical flow (.week-grid__events--flow has no
    // internal scroll region), so the active chip is already in the page scroll
    // — the nudge would needlessly move the page. Only the grid canvas needs it.
    // All-day + outside-window items are clipped y0 (not positioned on the
    // canvas), so scrolling their chip would jump the region to the window start
    // — skip them; they're shown in the lane / outside band.
    if (
      listMode ||
      eventIndex === null ||
      focusedItemIsAllDay ||
      focusedItemOutside
    )
      return;
    document
      .getElementById(eventOptionId(focusIndex, eventIndex))
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [
    focusIndex,
    eventIndex,
    eventOptionId,
    listMode,
    focusedItemIsAllDay,
    focusedItemOutside,
  ]);

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
          // A projection opens its underlying series (seriesTaskOf).
          else if (item?.kind === 'task') openTaskDialog(seriesTaskOf(item.task));
          return;
        }
        if (e.key === ' ' || e.key === 'Spacebar') {
          e.preventDefault();
          if (item?.kind === 'event') openEventDialog(item.event);
          // A read-only projection has no completion of its own — Space no-ops.
          else if (item?.kind === 'task' && !isRecurringProjection(item.task))
            void toggleTaskStatus(item.task);
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
            // A projection has no context actions — the menu is suppressed.
            else if (!isRecurringProjection(item.task)) void openTaskMenu(item.task, pos);
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
          // Pressing Enter on an empty cell opens the "Termin oder Aufgabe?"
          // chooser, anchored to that day so the form reflects what the user
          // is looking at.
          e.preventDefault();
          const focusedDay = days[focusIndex];
          const evs = eventsByDay.get(keyOf(focusedDay)) ?? [];
          if (evs.length > 0) {
            openEventDialog(evs[0]);
          } else {
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
      openCreateChooser,
      handleTab,
      clearEventIndex,
      requestDelete,
      toggleTaskStatus,
      seriesTaskOf,
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
        {/* Acts on the FOCUSED day, not on a fixed one — one button for the
            week instead of seven tab stops to reach the day you meant. */}
        <DayCheckInButton day={keyOf(days[focusIndex] ?? days[0])} />
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
          <div
            role="row"
            className={
              'week-grid__head' + (listMode ? ' week-grid__head--flow' : '')
            }
          >
            {/* Corner above the hour ruler (decorative — keeps the 7 day
                headers aligned over their columns). List mode has no ruler, so
                the corner is dropped and the head is a plain 7-day grid. */}
            {!listMode && (
              <div className="week-grid__corner" aria-hidden="true" />
            )}
            {days.map((day) => (
              <div
                key={day.toISOString()}
                role="columnheader"
                className="week-grid__col-head"
                aria-current={isSameDay(day, today) ? 'date' : undefined}
              >
                <span className="week-grid__dow">{fmt.format(day, 'EEE')}</span>
                <span className="week-grid__date">{fmt.format(day, 'd')}</span>
                {/* Decoration only — see the month twin. The day CELL's
                    accessible name carries the marker names; this column
                    header is not where a screen reader reads them. */}
                {symbolsFor(keyOf(day)) && (
                  <span className="week-grid__markers" aria-hidden="true">
                    {symbolsFor(keyOf(day))}
                  </span>
                )}
              </div>
            ))}
          </div>

          {allDayBars.length > 0 && (
            <div
              className={
                'week-grid__lane' + (listMode ? ' week-grid__lane--flow' : '')
              }
              aria-hidden="true"
              style={
                { '--lane-rows': laneRows } as React.CSSProperties
              }
              // Sighted-only affordance: the all-day lane is a drop target so a
              // bar can be dragged sideways to another day (its natural gesture —
              // the hour-grid cells below only cover timed drops). A drop over
              // another bar bubbles here, and the day is read from the cursor X.
              // SR users move all-day events via the Move/Copy dialog instead.
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
                const key = dayKeyAtClientX(e.clientX);
                if (key && key !== dragOverDayKey) setDragOverDayKey(key);
              }}
              onDragLeave={(e) => {
                if (
                  e.relatedTarget instanceof Node &&
                  e.currentTarget.contains(e.relatedTarget)
                ) {
                  return;
                }
                setDragOverDayKey(null);
              }}
              onDrop={(e) => {
                const key = dayKeyAtClientX(e.clientX);
                setDragOverDayKey(null);
                if (!key) return;
                e.preventDefault();
                const taskId =
                  e.dataTransfer.getData('text/aperio-task') || draggingTaskId;
                if (taskId) {
                  void rescheduleTaskByDrop(taskId, key);
                  return;
                }
                const dropped = readEventDrag(e.dataTransfer);
                if (dropped) handleEventDayDrop(dropped, key);
              }}
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
                  // GRID mode has a leading hour-ruler column, so day N → grid
                  // col N+1. LIST mode drops the ruler (pre-grid layout), so the
                  // 7 day columns start at 1 — day N → grid col N.
                  gridColumn: listMode
                    ? `${bar.startCol} / ${bar.endCol + 1}`
                    : `${bar.startCol + 1} / ${bar.endCol + 2}`,
                  gridRow: String(bar.lane + 1),
                };
                if (color.hex) style['--event-color'] = color.hex;
                return (
                  <div
                    key={eventInstanceKey(bar.event)}
                    className={
                      'week-allday-bar' +
                      (isBarFocused ? ' week-allday-bar--focused' : '') +
                      (bar.event.cancelled ? ' week-allday-bar--cancelled' : '') +
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

          <div
            role="row"
            className={
              'week-grid__body' + (listMode ? ' week-grid__body--flow' : '')
            }
          >
            {/* Hour ruler — the hour numbers, read off the grid instead of the
                chips. aria-hidden; the time is in each chip's accessible label.
                The scale is 24h tall, top-aligned with each cell's events area
                so the numbers line up with the gridlines. Grid mode only — the
                compact list reads the time off each chip's label, not a ruler. */}
            {!listMode && (
              <div className="week-grid__ruler" aria-hidden="true">
                {/* The scale height scales to the visible window via the inline
                    style (a React custom prop in calc() can't silently fail). */}
                <div
                  className="week-grid__ruler-scale"
                  style={{ height: gridHeight }}
                >
                  {rulerHours.map((h) => (
                    <span
                      key={h}
                      className="week-grid__ruler-hour"
                      style={{
                        top: `${((h * 60 - dayStartMin) / windowMin) * 100}%`,
                      }}
                    >
                      {String(h).padStart(2, '0')}
                    </span>
                  ))}
                </div>
              </div>
            )}
            {days.map((day, i) => {
              const dayKey = keyOf(day);
              const merged = dayItemsByDay.get(dayKey);
              const timedItems = merged?.timed ?? [];
              const untimedTasks = merged?.untimed ?? [];
              // Hour-grid placement for this day (computed once in slotByDay,
              // shared with the outside-band builder). Each non-all-day timed
              // item maps to a PositionedSpan: 'in' → absolute slot inside the
              // window canvas; 'before'/'after' → clipped, shown in the cell's
              // outside band. All-day chips stay clipped in the lane (no slot).
              // DOM order (and therefore SR/keyboard nav) is untouched.
              const slotByIdx = slotByDay.get(dayKey);
              const outsideBands = outsideBandsByDay.get(dayKey);
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
                  aria-label={[
                    t('views.week.dayAnnounce', {
                      day: fmt.format(day, 'PPPP'),
                      // Events + tasks (timed + untimed) — every chip in the cell.
                      count: timedItems.length + untimedTasks.length,
                    }),
                    spokenFor(dayKey),
                  ]
                    .filter(Boolean)
                    .join('. ')}
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
                    // Double-click on an empty part of the day opens the
                    // "Termin oder Aufgabe?" chooser, anchored to it. Skip
                    // clicks on a chip (events/tasks are draggable and have
                    // their own double-click → editor). Keyboard equivalent:
                    // Enter on the focused day.
                    if (
                      (e.target as HTMLElement).closest('[draggable="true"]')
                    ) {
                      return;
                    }
                    openCreateChooser(keyOf(day));
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
                    if (!dropped) return;
                    // In grid mode the cell IS the hour canvas, so where the
                    // pointer let go names a time. This was the missing half:
                    // an event could be moved between days and never to a
                    // different hour, which is most of what dragging one in a
                    // week planner is for. List mode has no hour geometry, so
                    // it stays a day-only move.
                    let minute: number | null = null;
                    if (!listMode) {
                      const box = e.currentTarget.getBoundingClientRect();
                      const fraction =
                        box.height > 0 ? (e.clientY - box.top) / box.height : 0;
                      minute = dropMinuteInWindow(fraction, {
                        startMin: dayStartMin,
                        endMin: dayEndMin,
                      });
                    }
                    handleEventDayDrop(dropped, dayKey, minute);
                  }}
                >
                  {/* Events/tasks before the window start — a compact band at
                      the TOP of the cell (sighted view; the a11y is the clipped
                      chips in the listbox). Grid mode only. */}
                  {!listMode && (
                    <WeekOutsideBand
                      entries={outsideBands?.before ?? []}
                      edge="before"
                      label={t('views.day.outsideBefore', {
                        time: clockAt(dayStartMin, dayKey),
                      })}
                    />
                  )}
                  <ul
                    role="list"
                    className={
                      'week-grid__events' +
                      (listMode ? ' week-grid__events--flow' : '')
                    }
                    // Grid mode: the canvas height + gridline offset scale to the
                    // visible window (inline, so a React custom prop in calc()
                    // can't silently fail — the bug a8c7d65 fixed in DayView).
                    // List mode: no positioned canvas — let --flow drive height.
                    style={
                      listMode
                        ? undefined
                        : {
                            height: gridHeight,
                            backgroundPositionY: gridLineOffset,
                          }
                    }
                    // Task drop ON the canvas → day + wall-clock time from the
                    // drop position (grid mode only; event drags bubble to the
                    // cell's day-move handler unchanged).
                    onDrop={
                      listMode ? undefined : (e) => dropTaskAtTime(e, dayKey)
                    }
                  >
                    {timedItems.map((item, itemIdx) => {
                      const isFocusedItem =
                        focused && eventIndex === itemIdx;
                      // In list mode the canvas is normal flow — no absolute
                      // slot. Each chip gets an inline min-height instead
                      // (events by duration, tasks by effort). In grid mode an
                      // 'in' item gets a positioned slot; an item entirely
                      // outside the window is clipped (its chip stays a navigable
                      // focus target — the cell's outside band is the sighted
                      // view).
                      const slot = listMode ? undefined : slotByIdx?.get(itemIdx);
                      const slotIn = slot?.placement === 'in';
                      const slotOut = slot != null && slot.placement !== 'in';
                      if (item.kind === 'task') {
                        const task = item.task;
                        // A read-only future occurrence of a recurring task — no
                        // drag/toggle/menu; it opens its series on activate.
                        const projection = isRecurringProjection(task);
                        // Pull the effective time-of-day for this row
                        // on this specific day — could come from either
                        // scheduled_time (the planned slot) or
                        // deadline_time (when this is the deadline day
                        // and only a deadline is set). The same helper
                        // backs taskTimeOnDay-based sorting upstream,
                        // so chips line up consistently.
                        const timeOnDay = taskTimeOnDay(task, dayKey);
                        // A planned block reads as a span, like an event's.
                        const endOnDay = taskEndTimeOnDay(task, dayKey);
                        const time = timeOnDay
                          ? endOnDay
                            ? t('views.timeRange', {
                                start: fmt.format(
                                  new Date(`${dayKey}T${timeOnDay}`),
                                  'p',
                                ),
                                end: fmt.format(
                                  new Date(`${dayKey}T${endOnDay}`),
                                  'p',
                                ),
                              })
                            : fmt.format(
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
                        const priorityGlyph = priorityMarker(task.priority, priorityScale);
                        const effortMod = visualEffortSizing
                          ? effortSizeModifier(task.effort)
                          : '';
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
                            className={
                              'week-grid__task-item' +
                              (slotIn ? ' week-grid__slot' : '') +
                              // Outside the visible window → clip the <li> (it
                              // stays a navigable focus target; the cell's
                              // outside band is the sighted view) instead of
                              // letting it flow static and corrupt the canvas.
                              (slotOut ? ' week-event--in-lane' : '') +
                              // Effort sizing applies to the timed task's <li>
                              // too: in GRID mode the effort class's min-height
                              // composes with the absolute slot to give a
                              // large-effort task a taller block at its time; in
                              // LIST mode it sizes the normal-flow row. Events
                              // are sized by duration instead, so this is
                              // task-only.
                              (effortMod
                                ? ` week-task--effort-${effortMod}`
                                : '')
                            }
                            style={
                              slotIn && slot
                                ? slotStyle(
                                    slot,
                                    slotMinFraction *
                                      effortSlotFactor(effortMod),
                                  )
                                : undefined
                            }
                          >
                            <span
                              id={eventOptionId(i, itemIdx)}
                              className={
                                'week-task week-task--timed' +
                                (isFocusedItem ? ' week-task--focused' : '') +
                                (projection ? ' week-task--projection' : '') +
                                (draggingTaskId === task.id
                                  ? ' week-task--dragging'
                                  : '') +
                                ` week-task--${task.status.replace('_', '-')}`
                                // Effort sizing lives ONLY on the load-bearing
                                // <li> above — `.week-task--effort-*` sets
                                // min-height AND padding-block, so doubling it
                                // here pushed the title toward clipping. The span
                                // fills the <li> via the `height:100%` rule.
                              }
                              // The aria-label carries the status as a
                              // word suffix so SR users hear it on focus
                              // — the visible marker is only there for
                              // sighted users. After Space the toggle
                              // hook also fires a live-region
                              // announcement, so confirmation lands
                              // either way.
                              aria-label={
                                taskChipAriaLabel(
                                  t,
                                  task,
                                  time,
                                  tasks,
                                  priorityScale,
                                ) +
                                (task.recurrence
                                  ? t('views.tasks.recurringOccurrence')
                                  : '')
                              }
                              aria-selected={isFocusedItem}
                              style={
                                color.hex
                                  ? ({
                                      '--event-color': color.hex,
                                    } as React.CSSProperties)
                                  : undefined
                              }
                              // A projection is read-only: not draggable, opens
                              // its series (never toggles/menus) on activate.
                              draggable={!projection}
                              onDragStart={
                                projection
                                  ? undefined
                                  : (ev) => {
                                      setTaskDrag(
                                        ev.dataTransfer,
                                        task,
                                        tasks.filter((c) => c.parent_id === task.id),
                                      );
                                      setDraggingTaskId(task.id);
                                      dragOriginRef.current = {
                                        x: ev.clientX,
                                        y: ev.clientY,
                                      };
                                    }
                              }
                              onDragEnd={
                                projection
                                  ? undefined
                                  : () => {
                                      dragOriginRef.current = null;
                                      setDraggingTaskId(null);
                                      setDragOverDayKey(null);
                                    }
                              }
                              // Mouse: single click only focuses the day (the
                              // click bubbles to the cell); double click opens
                              // the editor. The marker (below) stops the bubble
                              // so toggling the checkbox doesn't move the anchor.
                              onDoubleClick={(e) => {
                                e.stopPropagation();
                                openTaskDialog(seriesTaskOf(task));
                              }}
                              onContextMenu={(ev) => {
                                ev.preventDefault();
                                ev.stopPropagation();
                                if (!projection) void openTaskMenu(task);
                              }}
                            >
                              {/* GRID mode: time is read off the hour-ruler, so
                                  the chip is title-only (a short chip's one
                                  visible line is the title, not clipped). LIST
                                  mode gates the ruler off, so restore a small
                                  visible start time here. aria-hidden — the full
                                  time already lives in the aria-label, so this
                                  must not double-announce. */}
                              {listMode && time && (
                                <span
                                  className="week-task__time"
                                  aria-hidden="true"
                                >
                                  {time}
                                </span>
                              )}
                              <span className="week-task__body">
                                <span
                                  className="week-task__check"
                                  aria-hidden="true"
                                  onClick={
                                    projection
                                      ? undefined
                                      : (ev) => {
                                          ev.stopPropagation();
                                          void toggleTaskStatus(task);
                                        }
                                  }
                                >
                                  {projection ? '↻' : statusMarker(task.status)}
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
                      const span = multiDayInfo(ev, day);
                      // A TIMED event that crosses midnight (`span` non-null)
                      // shows the THIS-day CLAMPED portion, not the absolute
                      // instants — so the next-day tail reads "00:00 – 01:00"
                      // (and the start day "23:00 – 24:00") instead of the
                      // confusing absolute "23:00 – 01:00". Single-day timed
                      // events keep the absolute start/end. Shared by the visible
                      // start time below + the aria range so they agree.
                      const { startStr: dayStartStr, endStr: dayEndStr } =
                        span && !ev.all_day
                          ? eventDayTimes(fmt, ev, day)
                          : {
                              startStr: fmt.format(new Date(ev.start), 'p'),
                              endStr: fmt.format(new Date(ev.end), 'p'),
                            };
                      // The chip shows only the title (time is read from the
                      // hour-grid + ruler); the label speaks the full start–end
                      // range so an SR user hears the DURATION.
                      const timeAria = ev.all_day
                        ? t('views.allDay')
                        : `${dayStartStr} – ${dayEndStr}`;
                      // The continuation (tail) chip of a TIMED cross-midnight
                      // event must NOT be draggable: `moveEventToDay` derives the
                      // move delta from the absolute START day, so dragging the
                      // day-N+1 chip would reschedule relative to day N — wrong.
                      // The start-day chip (dayIndex 1) keeps a well-defined
                      // anchor and stays draggable. (Blind users use Move/Copy;
                      // this only fixes the mouse affordance.) All-day chips are
                      // clipped out of the cell — the lane bar carries their drag.
                      const isTimedTail =
                        !ev.all_day && span != null && span.dayIndex > 1;
                      // Color label is purely visual — it's a visible
                      // accent strip on the chip, not extra information
                      // an SR user needs spoken. The calendar / list
                      // affiliation stays in the label.
                      const ariaBase = t('views.week.eventLabel', {
                        title: ev.title,
                        time: timeAria,
                        calendar: cal?.name ?? '—',
                      });
                      // What this chip stands for, if it stands for more than
                      // itself. The count comes from the group, so a copy in a
                      // switched-off calendar is counted too.
                      const groupRow = groupRows.get(
                        `${eventInstanceKey(ev)}@${dayKey}`,
                      );
                      const groupSuffix = groupRow?.group
                        ? groupRow.diverged
                          ? t('views.eventGroupDivergedSuffix', {
                              count: groupRow.otherMembers,
                            })
                          : t('views.eventGroupSuffix', {
                              count: groupRow.otherMembers,
                              calendars: groupRow.calendarIds
                                .map((id) => calendarById.get(id)?.name ?? id)
                                .join(', '),
                            })
                        : '';
                      const badge = groupRow ? groupBadge(groupRow) : null;
                      const aria =
                        (span
                          ? ariaBase +
                            t('views.multiDaySuffix', {
                              day: span.dayIndex,
                              total: span.totalDays,
                            })
                          : ariaBase) +
                        groupSuffix +
                        (ev.cancelled ? t('views.eventCancelledSuffix') : '');
                      // All-day events are visualised by the lane above in BOTH
                      // modes; their per-day chip stays in the listbox as the
                      // aria-activedescendant target but is clipped out of the
                      // visual flow (`--in-lane`) so the cell only shows timed
                      // events. The bar's focused state is driven from here via
                      // `focusedEvId`. A timed event gets a duration-scaled
                      // min-height in LIST mode via eventBlockFactor; all-day
                      // events get none (they're clipped anyway).
                      const evSpan = ev.all_day
                        ? null
                        : eventSpanForDay(
                            new Date(ev.start),
                            new Date(ev.end),
                            day,
                          );
                      const listHeight =
                        listMode && evSpan
                          ? `${
                              eventBlockFactor(evSpan.endMin - evSpan.startMin) *
                              WEEK_LIST_BLOCK_BASE_REM
                            }rem`
                          : undefined;
                      return (
                        <li
                          key={eventInstanceKey(ev)}
                          role="listitem"
                          className={
                            slotIn
                              ? 'week-grid__slot'
                              : // Outside the visible window → clip the <li> (it
                                // stays a navigable focus target; the cell's
                                // outside band is the sighted view). All-day
                                // events clip via the inner span's --in-lane
                                // instead (the lane is their visible home), so
                                // only windowed timed events clip here.
                                slotOut
                                ? 'week-event--in-lane'
                                : undefined
                          }
                          style={
                            slotIn && slot
                              ? slotStyle(slot, slotMinFraction)
                              : undefined
                          }
                        >
                          <span
                            id={eventOptionId(i, itemIdx)}
                            className={
                              'week-event' +
                              (isFocusedItem ? ' week-event--focused' : '') +
                              (span ? ' week-event--multiday' : '') +
                              (ev.cancelled ? ' week-event--cancelled' : '') +
                              // The `--in-lane` clip applies in BOTH modes: the
                              // all-day lane carries the visible bar in grid AND
                              // list mode (pre-grid behaviour), so an all-day
                              // event is never a plain row in the week list — it
                              // lives only in the lane above.
                              (ev.all_day ? ' week-event--in-lane' : '')
                            }
                            aria-label={aria}
                            aria-selected={isFocusedItem}
                            // List mode clips the title to the duration height —
                            // give sighted users the full title on hover (SR users
                            // already get it from aria-label).
                            title={listMode ? ev.title : undefined}
                            draggable={!isTimedTail}
                            onDragStart={
                              isTimedTail
                                ? undefined
                                : (dev) => {
                                    // Drag onto a sidebar calendar row to move
                                    // the event there (mouse affordance; the
                                    // keyboard/SR path is the Move/Copy dialog).
                                    setEventDrag(dev.dataTransfer, ev);
                                  }
                            }
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
                              {
                                ...(color.hex
                                  ? { '--event-color': color.hex }
                                  : {}),
                                // List mode: a STRICT duration-driven height on the
                                // chip itself, so the COLOURED block both fills the
                                // space AND can't be inflated past its duration by a
                                // long title — the title wraps on one flow and is
                                // clipped (full text in the aria-label + the title
                                // tooltip). Grid mode leaves listHeight undefined
                                // (the slot drives height via height:100%).
                                ...(listHeight ? { height: listHeight } : {}),
                              } as React.CSSProperties
                            }
                          >
                            {/* GRID mode: time is read off the hour-ruler, so
                                the chip is title-only. LIST mode gates the ruler
                                off, so restore a small visible start time (or
                                "all day"). aria-hidden — the full start–end range
                                already lives in the aria-label. */}
                            {listMode && (
                              <span
                                className="week-event__time"
                                aria-hidden="true"
                              >
                                {ev.all_day ? t('views.allDay') : dayStartStr}
                              </span>
                            )}
                            <span className="week-event__title">{ev.title}</span>
                            {span && (
                              <span className="week-event__span">
                                {t('views.multiDayCompact', {
                                  day: span.dayIndex,
                                  total: span.totalDays,
                                })}
                              </span>
                            )}
                            {/* Folding was audible and invisible. `aria-hidden`
                                — the option's label says it in words. */}
                            {badge && (
                              <span
                                className={
                                  'week-event__group' +
                                  (groupRow?.diverged
                                    ? ' week-event__group--diverged'
                                    : '')
                                }
                                aria-hidden="true"
                              >
                                {badge}
                              </span>
                            )}
                          </span>
                        </li>
                      );
                    })}
                  </ul>
                  {/* Events/tasks after the window end — a compact band at the
                      BOTTOM of the cell (mirror of the before-band above the
                      canvas). Grid mode only. */}
                  {!listMode && (
                    <WeekOutsideBand
                      entries={outsideBands?.after ?? []}
                      edge="after"
                      label={t('views.day.outsideAfter', {
                        time: clockAt(dayEndMin, dayKey),
                      })}
                    />
                  )}
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
                    listMode={listMode}
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
              pendingEventDrop.minute,
            );
          }
        }}
        onSeries={() => {
          if (pendingEventDrop) {
            void performEventDrop(
              pendingEventDrop.event,
              pendingEventDrop.dayKey,
              'series',
              pendingEventDrop.minute,
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
    // Bucket each event into every visible day it covers (via
    // daysCoveredKeys) — otherwise the user would see day 1 of a vacation
    // and nothing on days 2..N (DESIGN tradeoff: visibility beats
    // compactness, a future iteration may replace the per-day chips with one
    // continuous bar in a dedicated all-day lane). This spreads multi-day
    // ALL-DAY events AND timed events that cross midnight (a 23:00→01:00
    // meeting lands on both the start day and the next), so the next day
    // shows its own clamped portion.
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
  listMode,
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
  /** GRID vs compact-LIST layout. Drives ONLY a visual modifier class on the
   *  task <ul>: in grid mode the untimed tasks are a full-width list pinned to
   *  the BOTTOM of the cell, below the hour canvas (per Toni's "the task band
   *  stays under the grid", spanning the column), with a footer separator. In
   *  list mode it stays the pre-grid bottom-pinned column. The chips' DOM order,
   *  option ids, indices, handlers and a11y are IDENTICAL either way — only the
   *  CSS layout changes. */
  listMode: boolean;
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
  const { visualEffortSizing, priorityScale } = useTaskCascadeEnabled();
  if (tasks.length === 0) return null;
  return (
    <ul
      className={
        'week-grid__tasks' +
        // GRID-mode only: a footer separator so the bottom-pinned full-width
        // task list reads as distinct from the canvas above. List mode keeps the
        // pre-grid column, so no modifier there. Purely visual — the DOM order /
        // option ids / Tab nav are unchanged in both.
        (listMode ? '' : ' week-grid__tasks--grid')
      }
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
        const priorityGlyph = priorityMarker(task.priority, priorityScale);
        const effortMod = visualEffortSizing
          ? effortSizeModifier(task.effort)
          : '';
        // A read-only future occurrence of a recurring task — a preview only:
        // no drag/complete/menu, and it opens its underlying series (resolved
        // from allTasks) on activate.
        const projection = isRecurringProjection(task);
        const seriesTask = projection
          ? allTasks.find((x) => x.id === recurringSeriesTaskId(task.id)) ?? task
          : task;
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
                (projection ? ' week-task--projection' : '') +
                ` week-task--${task.status.replace('_', '-')}` +
                (isBy ? ' week-task--by' : '') +
                (draggingTaskId === task.id
                  ? ' week-task--dragging'
                  : '') +
                (effortMod ? ` week-task--effort-${effortMod}` : '')
              }
              aria-selected={isFocused}
              // A projection is read-only: not draggable, opens its series
              // (never toggles/menus) on activate.
              draggable={!projection}
              onDragStart={projection ? undefined : (ev) => onDragStart(task, ev)}
              onDragEnd={projection ? undefined : onDragEnd}
              onDoubleClick={(e) => {
                e.stopPropagation();
                onOpen(seriesTask);
              }}
              onContextMenu={(ev) => {
                ev.preventDefault();
                ev.stopPropagation();
                if (!projection) onContextMenu(task);
              }}
              style={
                color.hex
                  ? ({ '--event-color': color.hex } as React.CSSProperties)
                  : undefined
              }
              aria-label={
                t(labelKey, {
                  title: task.title,
                  deadline: task.deadline_date
                    ? fmt.format(
                        new Date(`${task.deadline_date}T00:00:00`),
                        'PPP',
                      )
                    : '',
                  state,
                  priority: prioritySuffix(t, task.priority, priorityScale),
                  progress: subtaskProgressSuffix(t, task.id, allTasks),
                  assignee: assigneeSuffix(t, task.assignees),
                }) +
                subtaskParentSuffix(t, task, allTasks) +
                effortSuffix(t, task.effort) +
                (task.recurrence ? t('views.tasks.recurringOccurrence') : '')
              }
            >
              <span className="week-task__body">
                <span
                  className="week-task__check"
                  aria-hidden="true"
                  onClick={
                    projection
                      ? undefined
                      : (ev) => {
                          ev.stopPropagation();
                          onToggle(task);
                        }
                  }
                >
                  {projection ? '↻' : statusMarker(task.status)}
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
  scale: PriorityScale,
): string {
  const state = t(statusI18nKey(task.status));
  return (
    t('views.week.taskChipTimed', {
      title: task.title,
      time,
      state,
      priority: prioritySuffix(t, task.priority, scale),
      progress: subtaskProgressSuffix(t, task.id, allTasks),
      assignee: assigneeSuffix(t, task.assignees),
    }) +
    subtaskParentSuffix(t, task, allTasks) +
    effortSuffix(t, task.effort)
  );
}
