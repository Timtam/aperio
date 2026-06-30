import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isSameDay } from 'date-fns';

import { useAnnouncer } from '../../a11y/announcerContext';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { useDateFormat } from '../../intl/dateFormat';
import {
  labelsLookup,
  resolveEventColor,
  resolveTaskColor,
} from '../../intl/eventColor';
import { eventCoversDay, eventDayTimes, multiDayInfo } from '../../intl/multiDay';
import {
  isExpandedOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../../intl/recurrence';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { setEventDrag, setTaskDrag } from '../../state/moveActions';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useTaskListShowCompleted } from '../../state/useTaskListShowCompleted';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useTaskStatusToggle } from '../../state/useTaskStatusToggle';
import { useTasks } from '../../state/useTasks';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange } from '../../state/viewMath';
import { localDateKey } from '../../intl/dateKey';
import {
  filterTasksOnDay,
  isDeadlineChip,
  mergeDayItems,
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
import { duplicateEvent } from '../duplicateActions';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';
import type {
  CalendarEvent,
  ColorLabel,
  Task,
  TaskList,
} from '../../api/types';
import {
  eventBlockFactor,
  eventSpanForDay,
  layoutDayColumn,
  MINUTES_PER_DAY,
  minutesFromMidnight,
  type PositionedSpan,
  type TimedSpan,
} from '@aperio/shared';

/** Base block height (rem) a LIST-mode event row gets at `eventBlockFactor === 1`
 *  (a point / ≤1h event) — ≈ one natural row plus a little fill. The list-mode row
 *  uses a STRICT height (not min-height) of `factor × this`, with the time + title
 *  on one wrapping flow clipped to fit, so the row height reads DURATION at a
 *  glance and a long title can never inflate a short event past a long one. A 4h
 *  event is ~4× a 1h via the shared linear `eventBlockFactor` curve (floor 1, cap
 *  6). rem (not em) so the small row font doesn't shrink the scale; a touch taller
 *  than WeekView's 2.25rem because DayView rows are a bit taller. */
const DAY_LIST_BLOCK_BASE_REM = 2.5;

/** The slot's CSS `min-height` (1.2em, see `.day-list__slot`) as a fraction of
 *  the FULL-DAY canvas, so a floored late-night option can be clamped to stay
 *  on-canvas. With a narrower visible window the canvas is shorter, so the same
 *  absolute min-height is a LARGER fraction — callers scale this up via
 *  `slotStyle`'s `minFraction` arg (see the window-aware value in DayView).
 *  Matches WeekView's MIN_SLOT_FRACTION. */
const MIN_SLOT_FRACTION = 0.018;

/** Absolute placement of a timed chip's `<li>` inside the visible-window
 *  hour-grid (positioning is purely visual; DOM order is unchanged). `minFraction`
 *  is the floored option's min-height as a fraction of the CURRENT canvas (the
 *  window, which may be < 24h) — used by the TOP clamp that keeps a floored
 *  min-height option (a window-edge point) from extending below the canvas. */
function slotStyle(
  p: PositionedSpan,
  minFraction = MIN_SLOT_FRACTION,
): React.CSSProperties {
  const eh = Math.max(p.heightFraction, minFraction);
  const top = Math.min(p.topFraction, 1 - eh);
  return {
    position: 'absolute',
    top: `${top * 100}%`,
    height: `${p.heightFraction * 100}%`,
    left: `${(p.columnIndex / p.columnCount) * 100}%`,
    width: `${(1 / p.columnCount) * 100}%`,
  };
}

/** One chip in the "outside the visible hours" band (before/after the window).
 *  Holds just what the decorative band needs; the real a11y is the clipped
 *  listbox option this duplicates. */
interface OutsideBandEntry {
  key: string;
  title: string;
  /** Localised start time, e.g. "06:00" — so the band reads "06:00 Title". */
  time: string;
  colorHex?: string;
  onOpen: () => void;
}

/** Decorative band of the events/tasks that fall outside the visible day window
 *  — rendered above (before) / below (after) the hour-grid, mirroring the
 *  all-day band. `aria-hidden`: each entry is also a clipped, navigable listbox
 *  option (that's where the a11y lives), so the bars are `tabIndex={-1}` and
 *  exist only for sighted users. Returns null when empty. */
function OutsideBand({
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
      className={`day-grid__outside day-grid__outside--${edge}`}
      aria-hidden="true"
    >
      <span className="day-grid__outside-label">{label}</span>
      {entries.map((e) => (
        <button
          key={e.key}
          type="button"
          tabIndex={-1}
          className="day-grid__outside-bar"
          style={
            e.colorHex
              ? ({ '--event-color': e.colorHex } as React.CSSProperties)
              : undefined
          }
          onClick={e.onOpen}
        >
          {e.time && <span className="day-grid__outside-time">{e.time}</span>}
          <span className="day-grid__outside-title">{e.title}</span>
        </button>
      ))}
    </div>
  );
}

/**
 * Day view — flat listbox of the focused day's events.
 *
 * Phase 3 keeps it deliberately simple: the events of the active day are
 * rendered as `role="option"` items inside a `role="listbox"`, with
 * keyboard navigation via `aria-activedescendant`. The 15-minute slot
 * grid + slot focus from DESIGN.md section 3.3 returns alongside the
 * event-creation dialog in a later phase.
 *
 * Why listbox rather than a non-interactive list: with the listbox
 * pattern the screen reader stays in focus mode and reads the active
 * option as it changes — just like Week/Month view. A bare
 * `tabIndex=-1` section lacks a clear interactive role and lets NVDA
 * fall back to browse mode, which would break arrow navigation.
 */
export function DayView() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { anchor } = useViewState();
  const { openEventDialog, openTaskDialog, openMoveCopy, invalidateData } =
    useDialogState();

  const range = useMemo(() => visibleRange('day', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { tasks, taskListById } = useTasks();
  const { visualEffortSizing, dayViewMode, dayStartMin, dayEndMin } =
    useTaskCascadeEnabled();
  // Compact-list layout vs the proportional hour-grid. In list mode the timed
  // listbox is normal vertical flow (no positioned 24h canvas), the ruler +
  // all-day band are not rendered, and each option is sized inline instead of a
  // slot (events by a STRICT duration height, tasks by effort). The a11y model —
  // role=listbox/option, ids, aria-activedescendant, keyboard, labels — is
  // byte-for-byte identical to grid mode.
  const listMode = dayViewMode === 'list';
  // Visible day window (synced pref). The hour-grid spans [dayStartMin, dayEndMin]
  // instead of a fixed 0–24, so the canvas height + ruler shrink to the window
  // and timed items are positioned relative to it (see slotByIdx / layoutDayColumn).
  // Default 0/1440 reproduces the full day exactly. `--day-hours` drives the CSS
  // height; `--day-grid-line-frac` shifts the hourly gridline pattern so the lines
  // land on whole hours even when the window starts on a half-hour.
  const windowMin = Math.max(1, dayEndMin - dayStartMin);
  const dayHours = windowMin / 60;
  const gridLineFrac = ((60 - (dayStartMin % 60)) % 60) / 60;
  // The slot min-height (MIN_SLOT_FRACTION of the FULL day) as a fraction of the
  // current (possibly narrower) window, so a floored point keeps its on-canvas
  // clamp at any window size. Capped so a very narrow window stays sane.
  const slotMinFraction = Math.min(
    0.5,
    (MIN_SLOT_FRACTION * MINUTES_PER_DAY) / windowMin,
  );
  // Whole-hour ruler ticks inside the window (e.g. 7…22 for a 7–23 window).
  const rulerHours = useMemo(() => {
    const out: number[] = [];
    for (let h = Math.ceil(dayStartMin / 60); h * 60 < dayEndMin; h += 1) {
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
  const { colorLabels, sectionColorById, sectionsByList, loadSections } =
    useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

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

  // Pick up multi-day all-day events on every day of their span — see
  // intl/multiDay for the rationale.
  const dayEvents = useMemo(
    () => events.filter((ev) => eventCoversDay(ev, anchor)),
    [events, anchor],
  );

  // Tasks visible on this day (§9.4): scheduled, On-deadline, or
  // By-deadline window. Same filter as WeekView, including the
  // per-list "show completed" sidebar toggle.
  const dayTasks = useMemo(
    () =>
      filterTasksOnDay(
        tasks,
        localDateKey(anchor),
        shouldShowCompletedForList,
        meFor,
      ),
    [tasks, anchor, shouldShowCompletedForList, meFor],
  );

  // Split tasks into "timed" (carry a deadline_time on this specific
  // day) and "untimed" (everything else). Timed tasks slot into the
  // events listbox sorted by time so a 14:00 task deadline appears
  // between a 13:00 meeting and a 15:00 standup — the bug fix the
  // user asked for. Untimed tasks still render in the dedicated
  // section below the listbox.
  const dayKey = useMemo(() => localDateKey(anchor), [anchor]);
  const { timedItems, untimedTasks } = useMemo(() => {
    const { timed, untimed } = mergeDayItems(
      dayEvents,
      dayTasks,
      dayKey,
      (ev) => new Date(ev.start).getTime(),
    );
    return { timedItems: timed, untimedTasks: untimed };
  }, [dayEvents, dayTasks, dayKey]);

  // Hour-grid placement: each timed item gets an absolute slot (top/height by
  // start + duration, side-by-side on overlap) inside the 24h canvas. Keyed by
  // the item's index so the chip map applies it; DOM order (and therefore
  // SR/keyboard nav) is untouched. Mirrors WeekView's per-column logic for the
  // single day column.
  const slotByIdx = useMemo(() => {
    const map = new Map<number, PositionedSpan>();
    const spans: TimedSpan[] = [];
    const slotIdxs: number[] = [];
    timedItems.forEach((item, idx) => {
      let s: TimedSpan | null = null;
      if (item.kind === 'event') {
        // All-day events stay out of the timed grid (they have no meaningful
        // hour placement); they still render in DOM order without a slot.
        if (!item.event.all_day) {
          s = eventSpanForDay(
            new Date(item.event.start),
            new Date(item.event.end),
            anchor,
          );
        }
      } else {
        // A timed task is a zero-duration point; an unparseable time falls back
        // to midnight so it ALWAYS gets a slot and never flows static inside
        // the positioned canvas (which would corrupt the grid).
        const m = minutesFromMidnight(taskTimeOnDay(item.task, dayKey) ?? '');
        s = { startMin: m ?? 0, endMin: m ?? 0 };
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
    return map;
  }, [timedItems, anchor, dayKey, dayStartMin, dayEndMin]);

  // Items ENTIRELY outside the visible window (placement 'before' / 'after')
  // aren't placed on the canvas: their listbox option is clipped (visually
  // hidden but still navigable, reading the real time off its label — exactly
  // like an all-day option), and the SIGHTED representation is a compact band
  // above (before) / below (after) the grid. Mirrors the .day-grid__allday
  // pattern. Built in time order; events AND timed tasks both land here.
  const outsideBands = useMemo(() => {
    const before: OutsideBandEntry[] = [];
    const after: OutsideBandEntry[] = [];
    timedItems.forEach((item, idx) => {
      const slot = slotByIdx.get(idx);
      if (!slot || slot.placement === 'in') return;
      let entry: OutsideBandEntry;
      if (item.kind === 'event') {
        const ev = item.event;
        // Cross-midnight event: its tail on day 2 runs 00:00–…, so show the
        // clamped THIS-DAY start (like the grid chip + aria-label do), not the
        // misleading absolute start.
        const startStr = multiDayInfo(ev, anchor)
          ? eventDayTimes(fmt, ev, anchor).startStr
          : fmt.format(new Date(ev.start), 'p');
        entry = {
          key: `ev-${ev.id}`,
          title: ev.title,
          time: startStr,
          colorHex: resolveEventColor(ev, calendarById, labelById).hex ?? undefined,
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
          onOpen: () => openTaskDialog(task),
        };
      }
      (slot.placement === 'before' ? before : after).push(entry);
    });
    return { before, after };
  }, [
    timedItems,
    slotByIdx,
    anchor,
    calendarById,
    labelById,
    taskListById,
    sectionColorById,
    dayKey,
    fmt,
    openEventDialog,
    openTaskDialog,
  ]);

  // Window-boundary time as a localised "HH:MM" for the outside-band labels.
  // 1440 is the end-of-day sentinel (no real Date), so spell it literally.
  const clockAt = (min: number): string => {
    if (min >= MINUTES_PER_DAY) return '24:00';
    const hh = String(Math.floor(min / 60)).padStart(2, '0');
    const mm = String(min % 60).padStart(2, '0');
    return fmt.format(new Date(`${dayKey}T${hh}:${mm}:00`), 'p');
  };

  // All-day events have no hour placement, so they get no slot — their <li>
  // stays a (visually-hidden) option in the listbox for SR/keyboard, while the
  // sighted representation is this band above the grid. Mirrors WeekView's
  // clipped `--in-lane` option + visible all-day lane.
  const allDayEvents = useMemo(
    () =>
      timedItems.flatMap((it) =>
        it.kind === 'event' && it.event.all_day ? [it.event] : [],
      ),
    [timedItems],
  );

  const [focusIndex, setFocusIndex] = useState(0);

  // If the day changes (or events arrive) and the previous focus index
  // is past the end of the new list, snap back to the last valid item.
  useEffect(() => {
    if (focusIndex >= timedItems.length) {
      setFocusIndex(Math.max(0, timedItems.length - 1));
    }
  }, [timedItems.length, focusIndex]);

  // Loading indicator is gated on `showLoading` (the deferred
  // variant), not the raw `loading` flag. That way a sub-200ms
  // local fetch — which is what happens whenever the user switches
  // views with cached data — never flashes "Lädt …". Only genuine
  // waits (CalDAV cold start, slow iCal feed) cross the threshold
  // and surface the indicator + the SR announcement.
  const showLoading = useDeferredLoading(loading);
  useEffect(() => {
    if (showLoading) announce(t('views.loading'));
  }, [showLoading, announce, t]);

  const idPrefix = useId();
  const itemId = useCallback(
    (i: number) => `${idPrefix}-item-${i}`,
    [idPrefix],
  );

  // The timed list is now a 24h-tall internal scroll region, and
  // aria-activedescendant (unlike a real DOM .focus()) does NOT auto-scroll the
  // active option into view — so an option on a scrolled-away hour would be
  // off-screen for sighted / low-vision keyboard users. Scroll it into view
  // whenever the active option (focusIndex) changes. Mirrors WeekView.
  useEffect(() => {
    // List mode is a normal vertical flow (.day-grid--flow has no internal
    // scroll region), so the active option is already in the page scroll — the
    // nudge would needlessly move the page. Only the grid's 24h canvas needs it.
    if (listMode || timedItems.length === 0) return;
    document
      .getElementById(itemId(focusIndex))
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    // `dayKey` is a dep so switching to a different day always re-scrolls the
    // active option into view, even when the new day has the same item count
    // and focusIndex is unchanged (otherwise the effect wouldn't re-run).
  }, [focusIndex, itemId, timedItems.length, dayKey, listMode]);
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

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
        // The DeleteEventScope / Confirm dialogs are local view state,
        // not part of DialogState, so closing them won't trigger a
        // refetch automatically. Bump the data version ourselves so
        // useEvents re-reads after the mutation lands on the server.
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

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const focusedItem = timedItems[focusIndex];
      // Ctrl+D duplicates the focused event in place; Shift+M opens
      // the move/copy dialog. Bare keys cover the rest of the listbox
      // navigation. Both shortcuts are event-only — duplicating a
      // task deadline doesn't have a clear meaning, and Move/Copy of
      // tasks lives in TaskView.
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          if (focusedItem?.kind === 'event') {
            const ev = focusedItem.event;
            void duplicateEvent(ev).then(() =>
              announce(
                t('actions.duplicated', { title: ev.title }),
              ),
            );
          }
        }
        return;
      }
      if (e.altKey) return;
      if (e.shiftKey && e.key.toLowerCase() === 'm') {
        e.preventDefault();
        if (focusedItem?.kind === 'event') {
          openMoveCopy({ kind: 'event', event: focusedItem.event });
        }
        return;
      }
      if (timedItems.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, timedItems.length - 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          setFocusIndex((i) => Math.max(i - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          setFocusIndex(0);
          return;
        case 'End':
          e.preventDefault();
          setFocusIndex(timedItems.length - 1);
          return;
        case 'Enter': {
          // Enter always opens the editor — for events the
          // EventDialog, for tasks the TaskDialog.
          e.preventDefault();
          if (focusedItem?.kind === 'event') {
            openEventDialog(focusedItem.event);
          } else if (focusedItem?.kind === 'task') {
            openTaskDialog(focusedItem.task);
          }
          return;
        }
        case ' ':
        case 'Spacebar': {
          // Space opens events (no other meaningful action) but
          // toggles done/open on tasks. Matches TaskView's existing
          // Space-to-check convention and the user-visible
          // checkbox marker on the chip.
          e.preventDefault();
          if (focusedItem?.kind === 'event') {
            openEventDialog(focusedItem.event);
          } else if (focusedItem?.kind === 'task') {
            void toggleTaskStatus(focusedItem.task);
          }
          return;
        }
        case 'Delete':
        case 'Backspace': {
          e.preventDefault();
          // Tasks go through their own delete flow via TaskDialog;
          // the listbox Delete shortcut only nukes events, matching
          // WeekView semantics.
          if (focusedItem?.kind === 'event') {
            requestDelete(focusedItem.event);
          }
          return;
        }
        case 'ContextMenu':
        case 'F10': {
          if (e.key === 'F10' && !e.shiftKey) return;
          // Shift+F10 / Menu key: open the chip context menu near
          // the focused row, mirroring the platform convention.
          e.preventDefault();
          if (focusedItem) {
            const target = e.currentTarget as HTMLElement;
            const id = itemId(focusIndex);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            const pos = rect
              ? { x: rect.left, y: rect.bottom }
              : undefined;
            if (focusedItem.kind === 'event') {
              void openEventMenu(focusedItem.event, pos);
            } else {
              void openTaskMenu(focusedItem.task, pos);
            }
          }
          return;
        }
        default:
          return;
      }
    },
    [
      timedItems,
      focusIndex,
      openEventDialog,
      openTaskDialog,
      openMoveCopy,
      announce,
      t,
      requestDelete,
      toggleTaskStatus,
      openEventMenu,
      openTaskMenu,
      itemId,
    ],
  );

  const today = useMemo(() => new Date(), []);
  const isToday = isSameDay(today, anchor);

  return (
    <section className="view view--day" aria-label={fmt.format(anchor, 'PPPP')}>
      <header className="view__header">
        <h2 aria-current={isToday ? 'date' : undefined}>
          {fmt.format(anchor, 'PPPP')}
        </h2>
      </header>

      {showLoading && (
        <p className="view__loading" aria-hidden="true">
          {t('views.loading')}
        </p>
      )}

      {/* The timed list is a 24h-tall positioned hour-canvas (see .day-list in
          styles.css). A leading hour-ruler carries the hour numbers (00–23) so
          the time reads off the grid; the canvas + ruler scroll together as one
          internal region. Purely visual — the listbox keeps its role, IDs,
          aria-activedescendant and keyboard handlers unchanged. */}
      {/* All-day events sit above the hour-grid (they have no hour position).
          Decorative: each event also stays a clipped option in the listbox
          below, so SR/keyboard navigation reaches it on every day it covers. */}
      {!listMode && allDayEvents.length > 0 && (
        <div className="day-grid__allday" aria-hidden="true">
          {allDayEvents.map((ev) => {
            const color = resolveEventColor(ev, calendarById, labelById);
            return (
              <button
                key={`allday-${ev.id}`}
                type="button"
                tabIndex={-1}
                className="day-grid__allday-bar"
                style={
                  color.hex
                    ? ({ '--event-color': color.hex } as React.CSSProperties)
                    : undefined
                }
                onClick={() => openEventDialog(ev)}
              >
                <span className="day-grid__allday-title">{ev.title}</span>
              </button>
            );
          })}
        </div>
      )}

      {/* Events/tasks before the window start — a compact band above the grid
          (sighted view; the a11y is the clipped options in the listbox). */}
      {!listMode && (
        <OutsideBand
          entries={outsideBands.before}
          edge="before"
          label={t('views.day.outsideBefore', { time: clockAt(dayStartMin) })}
        />
      )}

      <div
        className={'day-grid' + (listMode ? ' day-grid--flow' : '')}
        style={
          {
            '--day-hours': dayHours,
            '--day-grid-line-frac': gridLineFrac,
          } as React.CSSProperties
        }
      >
        {/* Hour ruler — the hour numbers, read off the grid instead of the
            chips. aria-hidden; the time stays in each option's accessible
            label. The scale is 24h tall, top-aligned with the canvas so the
            numbers line up with the gridlines. Grid mode only — the compact
            list reads the time off each option's label, not a ruler. */}
        {!listMode && (
          <div className="day-grid__ruler" aria-hidden="true">
            <div className="day-grid__ruler-scale">
              {rulerHours.map((h) => (
                <span
                  key={h}
                  className="day-grid__ruler-hour"
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
        <ul
          ref={listRef}
          role="listbox"
          tabIndex={0}
          aria-label={t('views.day.eventList')}
          aria-activedescendant={
            timedItems.length > 0 ? itemId(focusIndex) : undefined
          }
          onKeyDown={handleKeyDown}
          className={'day-list' + (listMode ? ' day-list--flow' : '')}
        >
        {timedItems.length === 0 ? (
          <li role="presentation" className="day-list__empty">
            {t('views.day.empty')}
          </li>
        ) : (
          timedItems.map((item, i) => {
            const focused = i === focusIndex;
            // Absolute slot inside the 24h canvas (every timed item has one —
            // see slotByIdx). Positioning is purely visual; the <li> keeps its
            // option role, id, aria-selected and DOM position unchanged. In
            // list mode there's no canvas — the option flows normally and is
            // sized inline instead (events by a STRICT duration height, tasks by
            // effort), so suppress the slot here.
            const slot = listMode ? undefined : slotByIdx.get(i);
            // In-window → positioned canvas slot. Outside the window → the
            // option is clipped (the band above/below is the sighted view).
            const slotIn = slot?.placement === 'in';
            const slotOut = slot != null && slot.placement !== 'in';
            if (item.kind === 'task') {
              const task = item.task;
              // Pull the effective time-of-day via the shared helper;
              // it returns scheduled_time when on the scheduled day,
              // deadline_time when on the deadline day, with the
              // schedule winning on a same-day collision.
              const timeOnDay = taskTimeOnDay(task, dayKey);
              const timeStr = timeOnDay
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
              const state = t(statusI18nKey(task.status));
              const priorityGlyph = priorityMarker(task.priority);
              const effortMod = visualEffortSizing
                ? effortSizeModifier(task.effort)
                : '';
              return (
                <li
                  key={`task-${task.id}`}
                  id={itemId(i)}
                  role="option"
                  aria-selected={focused}
                  aria-label={
                    t('views.day.taskLabel', {
                      title: task.title,
                      time: timeStr,
                      state,
                      priority: prioritySuffix(t, task.priority),
                      progress: subtaskProgressSuffix(t, task.id, tasks),
                      assignee: assigneeSuffix(t, task.assignees),
                    }) +
                    subtaskParentSuffix(t, task, tasks) +
                    effortSuffix(t, task.effort)
                  }
                  className={
                    'day-list__item day-list__item--task' +
                    (focused ? ' day-list__item--focused' : '') +
                    ` day-list__item--${task.status.replace('_', '-')}` +
                    // Deliberately reuses the `day-task--effort-*` size family
                    // (not a `day-list__item--*` one) so both DayView task
                    // surfaces — this agenda row + the grid chip — resize
                    // identically by effort.
                    (effortMod ? ` day-task--effort-${effortMod}` : '') +
                    (slotIn ? ' day-list__slot' : '') +
                    (slotOut ? ' day-list__item--outside' : '')
                  }
                  style={{
                    ...(slotIn && slot ? slotStyle(slot, slotMinFraction) : {}),
                    ...(color.hex
                      ? ({ '--event-color': color.hex } as React.CSSProperties)
                      : {}),
                  }}
                  draggable
                  onDragStart={(dev) => {
                    setTaskDrag(
                      dev.dataTransfer,
                      task,
                      tasks.filter((c) => c.parent_id === task.id),
                    );
                  }}
                  onClick={() => setFocusIndex(i)}
                  onDoubleClick={(e) => {
                    e.stopPropagation();
                    openTaskDialog(task);
                  }}
                  onContextMenu={(ev) => {
                    ev.preventDefault();
                    ev.stopPropagation();
                    setFocusIndex(i);
                    void openTaskMenu(task);
                  }}
                >
                  {/* GRID mode: time is read off the hour-ruler, so the option
                      is title-only (a short option's one visible line is the
                      title, not clipped). LIST mode gates the ruler off, so
                      restore a small visible time-of-day. aria-hidden — the full
                      time already lives in the aria-label above. */}
                  {listMode && timeStr && (
                    <span className="day-list__time" aria-hidden="true">
                      {timeStr}
                    </span>
                  )}
                  <span className="day-list__title">
                    <span
                      className="day-task__marker day-task__marker--clickable"
                      aria-hidden="true"
                      // Mouse: clicking the marker toggles the task
                      // without selecting the row, so users don't
                      // have to round-trip through the dialog just
                      // to check something off.
                      onClick={(ev) => {
                        ev.stopPropagation();
                        void toggleTaskStatus(task);
                      }}
                    >
                      {statusMarker(task.status)}{' '}
                    </span>
                    {task.parent_id ? '↳ ' : ''}
                    {task.title}
                    {priorityGlyph && (
                      <span className="day-task__priority" aria-hidden="true">
                        {' '}
                        {priorityGlyph}
                      </span>
                    )}
                  </span>
                </li>
              );
            }
            const ev = item.event;
            const cal = calendarById.get(ev.calendar_id);
            const color = resolveEventColor(ev, calendarById, labelById);
            const span = multiDayInfo(ev, anchor);
            // A TIMED event that crosses midnight (`span` non-null) must show the
            // THIS-day clamped portion, not the absolute instants: the next-day
            // tail should read "00:00 – 01:00", not the confusing absolute
            // "23:00 – 01:00", and the start day "23:00 – 24:00". A single-day
            // timed event keeps the absolute start/end. (All-day events ignore
            // these — their visible row reads "all day".)
            const { startStr, endStr } =
              span && !ev.all_day
                ? eventDayTimes(fmt, ev, anchor)
                : {
                    startStr: fmt.format(new Date(ev.start), 'p'),
                    endStr: fmt.format(new Date(ev.end), 'p'),
                  };
            const ariaBase = t('views.day.eventLabel', {
              title: ev.title,
              start: startStr,
              end: endStr,
              calendar: cal?.name ?? '—',
            });
            const aria = span
              ? ariaBase +
                t('views.multiDaySuffix', {
                  day: span.dayIndex,
                  total: span.totalDays,
                })
              : ariaBase;
            // In LIST mode a timed event gets a STRICT duration-scaled height via
            // eventBlockFactor (so the height reads duration; a long title clips
            // rather than inflating a short event); an all-day event renders as a
            // plain row (no clip, no height). In grid mode this stays unused (the
            // slot drives geometry).
            const evSpan =
              listMode && !ev.all_day
                ? eventSpanForDay(new Date(ev.start), new Date(ev.end), anchor)
                : null;
            const listHeight = evSpan
              ? `${
                  eventBlockFactor(evSpan.endMin - evSpan.startMin) *
                  DAY_LIST_BLOCK_BASE_REM
                }rem`
              : undefined;
            return (
              <li
                key={ev.id}
                id={itemId(i)}
                role="option"
                aria-selected={focused}
                aria-label={aria}
                // List mode clips the visible title to the duration height — give
                // sighted users the full title on hover (SR users already get it
                // from the aria-label above).
                title={listMode ? ev.title : undefined}
                className={
                  'day-list__item' +
                  (focused ? ' day-list__item--focused' : '') +
                  (span ? ' day-list__item--multiday' : '') +
                  // Timed → absolute slot in the canvas. All-day in GRID mode →
                  // no slot, so clip the <li> (it stays a navigable option; the
                  // sighted view is the .day-grid__allday band above) instead of
                  // letting it flow static and collide with the 00:00 chips. In
                  // LIST mode there's no band, so the all-day option stays a
                  // plain visible row (no clip).
                  (slotIn
                    ? ' day-list__slot'
                    : (slotOut || (ev.all_day && !listMode))
                      ? slotOut
                        ? ' day-list__item--outside'
                        : ' day-list__item--allday'
                      : '')
                }
                style={{
                  ...(slotIn && slot ? slotStyle(slot, slotMinFraction) : {}),
                  // List mode: a STRICT duration-driven height (not min-height),
                  // so the row both fills the reserved space AND can't be inflated
                  // past its duration by a long title — the title wraps on one flow
                  // and is clipped (full text in the aria-label + the title
                  // tooltip). Grid mode leaves listHeight undefined (the slot drives
                  // height).
                  ...(listHeight ? { height: listHeight } : {}),
                  ...(color.hex
                    ? ({ '--event-color': color.hex } as React.CSSProperties)
                    : {}),
                }}
                draggable
                onDragStart={(dev) => {
                  setEventDrag(dev.dataTransfer, ev);
                }}
                onClick={() => setFocusIndex(i)}
                onDoubleClick={(dcev) => {
                  // Open the editor, mirroring the task chips (single click
                  // just moves focus; the keyboard path is Enter).
                  dcev.stopPropagation();
                  setFocusIndex(i);
                  openEventDialog(ev);
                }}
                onContextMenu={(cmev) => {
                  cmev.preventDefault();
                  cmev.stopPropagation();
                  setFocusIndex(i);
                  void openEventMenu(ev);
                }}
              >
                {/* GRID mode: time is read off the hour-ruler, so the option is
                    title-only. LIST mode gates the ruler off, so restore a small
                    visible start–end range (or "all day"). aria-hidden — the
                    full range already lives in the aria-label above. */}
                {listMode && (
                  <span className="day-list__time" aria-hidden="true">
                    {ev.all_day ? t('views.allDay') : `${startStr} – ${endStr}`}
                  </span>
                )}
                <span className="day-list__title">
                  {ev.title}
                  {span && (
                    <span className="day-list__span">
                      {' '}
                      {t('views.multiDayCompact', {
                        day: span.dayIndex,
                        total: span.totalDays,
                      })}
                    </span>
                  )}
                </span>
              </li>
            );
          })
        )}
        </ul>
      </div>

      {/* Events/tasks after the window end — a compact band below the grid
          (mirror of the before-band above). */}
      {!listMode && (
        <OutsideBand
          entries={outsideBands.after}
          edge="after"
          label={t('views.day.outsideAfter', { time: clockAt(dayEndMin) })}
        />
      )}

      {/* §9.4 untimed tasks — always BELOW the grid (per Toni: the task band
          stays under the grid, not above it). GRID mode styles them as a compact
          band (`variant="band"`), LIST mode as the pre-grid full-width section.
          The chips are natural-Tab-order buttons either way (NOT listbox
          options), so the reading order is events first, then tasks. Tasks with a
          concrete deadline_time were already interleaved with events in the
          listbox above (sorted by time), so only scheduled-only tasks and
          By-window intermediate days surface here. Click / Enter / Space opens
          the TaskDialog; status toggles via the marker / Space. */}
      {untimedTasks.length > 0 && (
        <DayUntimedTasks
          variant={listMode ? 'section' : 'band'}
          tasks={untimedTasks}
          dayKey={dayKey}
          allTasks={tasks}
          fmt={fmt}
          t={t}
          taskListById={taskListById}
          labelById={labelById}
          sectionColorById={sectionColorById}
          visualEffortSizing={visualEffortSizing}
          onToggle={toggleTaskStatus}
          onOpen={openTaskDialog}
          onContextMenu={openTaskMenu}
        />
      )}

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

/**
 * §9.4 untimed-task chips for the day surface. Rendered in one of two
 * places depending on `dayViewMode`:
 *
 *  - `variant="band"` (GRID mode): a compact horizontal flex-wrap bar
 *    ABOVE the hour-grid, styled like the all-day band — so a sighted
 *    user finds the untimed work at the top instead of scrolling past
 *    the whole 24h canvas.
 *  - `variant="section"` (LIST mode): the original dedicated section
 *    BELOW the list (unchanged pre-grid placement).
 *
 * The chips themselves are byte-for-byte identical between the two
 * variants — same `<button className="day-task">`, same a11y label
 * (incl. the effort suffix), same keyboard / mouse / drag / context
 * handlers. They are plain Tab-order buttons in BOTH cases (NOT part
 * of the listbox aria-activedescendant nav). Only the wrapping
 * container + its CSS differ. Factored here so the markup isn't
 * duplicated across the two render sites in DayView.
 */
function DayUntimedTasks({
  variant,
  tasks,
  dayKey,
  allTasks,
  fmt,
  t,
  taskListById,
  labelById,
  sectionColorById,
  visualEffortSizing,
  onToggle,
  onOpen,
  onContextMenu,
}: {
  variant: 'band' | 'section';
  tasks: Task[];
  dayKey: string;
  allTasks: Task[];
  fmt: ReturnType<typeof useDateFormat>;
  t: ReturnType<typeof useTranslation>['t'];
  taskListById: Map<string, TaskList>;
  labelById: Map<string, ColorLabel>;
  sectionColorById: Map<string, string>;
  visualEffortSizing: boolean;
  onToggle: (task: Task) => void | Promise<void>;
  onOpen: (task: Task) => void;
  onContextMenu: (
    task: Task,
    position?: { x: number; y: number },
  ) => void | Promise<void>;
}) {
  return (
    <section
      className={
        'day-tasks' + (variant === 'band' ? ' day-tasks--band' : '')
      }
      aria-label={t('views.day.tasksHeading')}
    >
      <h3 className="day-tasks__heading">{t('views.day.tasksHeading')}</h3>
      <ul className="day-tasks__list">
        {tasks.map((task) => {
          // "Due here" when the task is on this day because of its
          // deadline (not its scheduled day) — that chip is the
          // deadline marker ("fällig bis …"). A task scheduled today
          // stays a plain work chip even with a later deadline.
          const isBy = isDeadlineChip(task, dayKey);
          // The scheduled chip now also announces its deadline (the
          // deadline-day duplicate is suppressed in filterTasksOnDay), so
          // use the "fällig bis …" label whenever the task carries a
          // deadline — not only on a pure deadline marker.
          const hasDeadline = task.deadline_date != null;
          const labelKey = hasDeadline
            ? 'views.week.taskChipBy'
            : 'views.week.taskChip';
          // Visible deadline badge on the SCHEDULED chip (a deadline-only
          // marker, `isBy`, already sits on its own day so it needs none).
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
          const effortMod = visualEffortSizing
            ? effortSizeModifier(task.effort)
            : '';
          return (
            <li key={task.id} className="day-tasks__item">
              <button
                type="button"
                className={
                  'day-task' +
                  ` day-task--${task.status.replace('_', '-')}` +
                  (isBy ? ' day-task--by' : '') +
                  (effortMod ? ` day-task--effort-${effortMod}` : '')
                }
                // Default <button> would fire onClick on both
                // Space and Enter. We need different actions:
                // Space toggles done (matches the visual ☐/☑),
                // Enter opens the editor. Intercept here.
                onKeyDown={(ev) => {
                  if (ev.key === ' ' || ev.key === 'Spacebar') {
                    ev.preventDefault();
                    void onToggle(task);
                  } else if (ev.key === 'Enter') {
                    ev.preventDefault();
                    onOpen(task);
                  } else if (
                    ev.key === 'ContextMenu' ||
                    (ev.shiftKey && ev.key === 'F10')
                  ) {
                    ev.preventDefault();
                    const rect = (
                      ev.currentTarget as HTMLElement
                    ).getBoundingClientRect();
                    void onContextMenu(task, {
                      x: rect.left,
                      y: rect.bottom,
                    });
                  }
                }}
                onDoubleClick={(e) => {
                  e.stopPropagation();
                  onOpen(task);
                }}
                onContextMenu={(ev) => {
                  ev.preventDefault();
                  ev.stopPropagation();
                  void onContextMenu(task);
                }}
                // Drag-to-reschedule is a band-only affordance: the
                // pre-grid LIST section never carried it, so keep that
                // surface byte-for-byte (no drag) and only the compact
                // grid-mode band gets the draggable chip, matching how
                // the grid's other task chips drag.
                draggable={variant === 'band'}
                onDragStart={
                  variant === 'band'
                    ? (dev) => {
                        setTaskDrag(
                          dev.dataTransfer,
                          task,
                          allTasks.filter((c) => c.parent_id === task.id),
                        );
                      }
                    : undefined
                }
                style={
                  color.hex
                    ? ({
                        '--event-color': color.hex,
                      } as React.CSSProperties)
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
                    priority: prioritySuffix(t, task.priority),
                    progress: subtaskProgressSuffix(t, task.id, allTasks),
                    assignee: assigneeSuffix(t, task.assignees),
                  }) +
                  subtaskParentSuffix(t, task, allTasks) +
                  effortSuffix(t, task.effort)
                }
              >
                <span
                  className="day-task__marker day-task__marker--clickable"
                  aria-hidden="true"
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void onToggle(task);
                  }}
                >
                  {statusMarker(task.status)}
                </span>
                <span className="day-task__title">
                  {task.parent_id ? '↳ ' : ''}
                  {task.title}
                </span>
                {priorityGlyph && (
                  <span className="day-task__priority" aria-hidden="true">
                    {priorityGlyph}
                  </span>
                )}
                {deadlineBadge && (
                  <span className="day-task__deadline" aria-hidden="true">
                    {t('views.week.taskChipDeadlineBadge', {
                      deadline: deadlineBadge,
                    })}
                  </span>
                )}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
