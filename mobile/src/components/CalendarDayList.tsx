import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  PixelRatio,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';

import type {
  ColorLabel,
  DayGridItem,
  MultiDayInfo,
  PositionedSpan,
  Section,
  Task,
  TaskList,
  TimedSpan,
} from '@aperio/shared';
import {
  assigneeSuffix,
  daysCoveredKeys,
  effortSizeModifier,
  effortSuffix,
  eventBlockFactor,
  eventSpanForDay,
  expandAll,
  filterTasksOnDay,
  isDeadlineChip,
  layoutDayColumn,
  localDateKey,
  mergeDayItems,
  minutesFromMidnight,
  multiDayInfo,
  occurrenceIsoOf,
  prioritySuffix,
  seriesIdOf,
  statusI18nKey,
  statusMarker,
  subtaskParentSuffix,
  subtaskProgressSuffix,
  taskTimeOnDay,
} from '@aperio/shared';

import {
  Calendar,
  CalendarEvent,
  getEvents,
  listCalendars,
} from '../api/calendar';
import {
  deleteTask as apiDeleteTask,
  getSections,
  getTasks,
  listTaskLists,
} from '../api/client';
import { listColorLabels } from '../api/colorLabels';
import { useTabBarInset } from '../hooks/useTabBarInset';
import { resolveEventColor } from '../intl/eventColor';
import { resolveTaskColor, sectionColorMap } from '../intl/taskColor';
import type { RootStackParamList } from '../navigation/types';
import { useCacheReload } from '../state/cacheObserver';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { useCurrentUserByList } from '../state/currentUser';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import { readTaskBehaviour } from '../state/taskBehaviour';
import { applyTaskToggle, statusAnnounce } from '../state/taskToggle';
import { useTaskListShowCompleted } from '../state/useTaskListShowCompleted';
import { useThemedStyles, type ThemeColors } from '../theme';

// The shared, screen-reader-first calendar day list — the rendering + data
// engine behind both the Week and Month views (and any future day-range view).
// Given the visible `days` and the `range` covering them, it loads everything
// (calendars + palette + lists + tasks + sections), expands recurring events,
// and renders one accessible section per day: a header announcing the day's
// item count, then that day's all-day events, its timed events + timed tasks
// merged chronologically (mergeDayItems), then its untimed tasks. Behaviour
// parity with the desktop comes from reusing the SAME shared domain logic
// (expandAll, daysCoveredKeys/multiDayInfo, filterTasksOnDay/mergeDayItems/
// taskTimeOnDay). Both audiences: coloured dots for sighted users, the bound
// label's NAME folded into every accessible label (WCAG 1.4.1). Event rows
// offer edit + delete; task rows complete (shared status cascade) / edit /
// delete. The owning screen supplies the day window + the chrome (nav/header).

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Local Date at `key`'s `HH:MM[:SS]` time-of-day (for localized formatting). */
function buildTimeDate(key: string, time: string): Date {
  const [hh, mm, ss] = time.split(':').map((n) => Number(n));
  const [y, mo, d] = key.split('-').map((n) => Number(n));
  return new Date(y, mo - 1, d, hh ?? 0, mm ?? 0, ss ?? 0);
}

// ── Single-day hour-grid geometry ───────────────────────────────────────────
// Mirrors the desktop DayView/WeekView grid. Only the single-day caller
// (EventsScreen, dayLayout='grid') renders the timed lane as a 24h-tall
// positioned canvas; multi-day callers (Week/Month/Agenda) stay the linear
// list. The overlap math lives in the shared, tested `layoutDayColumn`.

// The hour-grid is laid out in absolute pixels, so — unlike free-flowing text —
// it does NOT grow when the OS font scale (iOS Dynamic Type / Android font size)
// enlarges the chip labels, which would clip them. Scale the grid geometry by
// the OS font scale so the whole canvas (and every absolutely-positioned chip,
// which derives from CANVAS_PX) grows proportionally with the text, keeping the
// single-line labels readable. This is the mobile twin of the desktop
// `--hour-px`→rem fix. Read once at module load (a font-scale change while the
// app runs is picked up on the next launch — RN reloads on most such changes).
const FONT_SCALE = PixelRatio.getFontScale();

/** Canvas pixels per hour → the 24h column is HOUR_PX*24 tall. */
const HOUR_PX = Math.round(48 * FONT_SCALE);
/** Full timed-canvas height in px (24h). */
const CANVAS_PX = HOUR_PX * 24;
/** Minimum rendered chip height so a short/zero-duration item stays legible. */
const MIN_SLOT_PX = Math.round(28 * FONT_SCALE);
/** Hour-ruler column width (carries the 00–23 numbers). */
const RULER_PX = Math.round(44 * FONT_SCALE);
/** Top padding (px) left above the auto-scroll target in GRID mode so the first
 *  event isn't flush to the very top edge — a little breathing room reads
 *  better. Purely visual; only the single-day grid auto-scroll uses it. */
const GRID_SCROLL_PAD_PX = 12;

// ── Single-day compact-list geometry (dayLayout='list') ──────────────────────
// The lighter alternative to the hour-grid: a chronological list where a timed
// EVENT block's STRICT height reflects its DURATION (via the shared, platform-
// agnostic `eventBlockFactor`) so a long meeting reads as a taller block — no
// absolute slot positioning, and a long title can't inflate a short event (the
// title clips to the height). Tasks keep their EFFORT sizing instead.

/** Base px a LIST-mode event block gets at `eventBlockFactor === 1` (a point /
 *  ≤1h event) — ≈ one base `row` height plus a little fill. The list-mode chip
 *  uses a STRICT height (not min-height) of `factor × this` with `overflow:
 *  'hidden'`, so the height reads DURATION at a glance and a long wrapping title
 *  can never inflate a short event past a long one (the title clips vertically;
 *  the dot, time/meta and delete affordance lay out horizontally and stay
 *  visible). The full title is always in the row's accessibilityLabel, and
 *  tapping the row opens the editor. Bumped 46 → 69 (~1.5×) to match the desktop
 *  WeekView bump for fuller vertical-space use. A 1h event ≈ 69px, a 4h ≈ 276px
 *  (4×), a 6h+ caps at ≈414px. */
const LIST_EVENT_BASE_PX = Math.round(69 * FONT_SCALE);

/** Per-effort slot-height FLOOR for a TIMED TASK in the hour-grid (gated on the
 *  visualEffortSizing pref). Passed as slotStyle's `floorPx`, so it raises BOTH
 *  the min-height AND the top-clamp — a large task near midnight stays fully
 *  on-canvas. small < neutral < large; medium == MIN_SLOT_PX, so a medium task
 *  keeps the unmodified slot floor (NEUTRAL, matching desktop where only the
 *  small/large effort classes exist). */
const GRID_TASK_EFFORT_PX = {
  small: Math.round(22 * FONT_SCALE),
  medium: MIN_SLOT_PX,
  large: Math.round(46 * FONT_SCALE),
} as const;

/** Timed event's clamped duration in minutes on `day` (for the list-view block
 *  height). Reuses the shared eventSpanForDay so it matches the grid's duration
 *  math. */
function eventDurationMinForDay(start: Date, end: Date, day: Date): number {
  const span = eventSpanForDay(start, end, day);
  return span.endMin - span.startMin;
}

/** Absolute placement of a timed chip inside the 24h canvas (purely visual;
 *  source order is unchanged). top/height by start+duration, left/width by the
 *  overlap column. A short span keeps a `floorPx` min-height so it stays tappable;
 *  a timed task passes its per-effort floor (GRID_TASK_EFFORT_PX) so a higher-
 *  effort task reads as a taller block. Events use the default MIN_SLOT_PX. */
function slotStyle(p: PositionedSpan, floorPx = MIN_SLOT_PX) {
  const height = Math.max(p.heightFraction * CANVAS_PX, floorPx);
  // Clamp the TOP (not the height) so a floored min-height chip near midnight
  // stays fully on-canvas at its full `floorPx` height. Clamping the height
  // instead would squeeze a 23:50 chip below the tap target — here it shifts up
  // by a few px and keeps its full height, matching the desktop's intent. The
  // clamp uses the SAME floor, so even a large-effort task at 23:50 fits.
  const top = Math.min(p.topFraction * CANVAS_PX, CANVAS_PX - floorPx);
  return {
    position: 'absolute' as const,
    top,
    height,
    left: `${(p.columnIndex / p.columnCount) * 100}%` as const,
    width: `${(1 / p.columnCount) * 100}%` as const,
  };
}

interface DayBucket {
  key: string;
  date: Date;
  allDay: CalendarEvent[];
  timed: DayGridItem<CalendarEvent, Task>[];
  untimed: Task[];
  count: number;
}

export interface CalendarDayListProps {
  /** The owning screen's navigation (for the editor routes + focus reload). */
  navigation: NativeStackNavigationProp<RootStackParamList>;
  /** The visible days (local midnights), in order. */
  days: Date[];
  /** The instant range covering `days` (for the event/task fetch + expansion). */
  range: { start: Date; end: Date };
  /** accessibilityLabel for the list (e.g. "Week grid" / "Month grid"). */
  gridLabel: string;
  /** Shown when the window has no events or tasks. */
  emptyText: string;
  /** i18n key for the per-day header announce (`{{day}}, {{count}} items`). */
  dayAnnounceKey: string;
  /**
   * Render the per-day header row (the date + item count). Default `true`.
   * The single-day view passes `false`: it already shows the date in its own
   * nav-bar heading, so the list's per-day header would be a redundant second
   * heading for a screen-reader user.
   */
  showDayHeaders?: boolean;
  /**
   * Single-day visual layout for the day's items. `undefined` (the default) =
   * the plain multi-day linear list — the Week/Month/Agenda callers pass nothing
   * and render byte-for-byte as before this prop existed.
   *
   * The single-day caller (EventsScreen) passes the synced `calendar.dayViewMode`
   * pref:
   *   - `'grid'` → proportional 24h hour-grid (chips placed by start time, sized
   *     by duration; tasks raised by effort). The multi-day callers stack days
   *     vertically where a 7×24h / 30×24h grid would be unusable, so they keep
   *     the linear list — hence single-day ONLY.
   *   - `'list'` → a compact chronological list: every timed EVENT block's
   *     STRICT height reflects its DURATION (eventBlockFactor; a long title
   *     clips rather than inflating it), every timed TASK keeps its effort
   *     sizing. No slot positioning.
   *
   * Purely visual in all three paths: every row keeps its `accessible`, role,
   * full `accessibilityLabel` (incl. the time range), tap/action handlers, and
   * chronological source order (all-day → timed → untimed). Only where chips are
   * *drawn* / how tall they are changes.
   */
  dayLayout?: 'grid' | 'list';
}

export function CalendarDayList({
  navigation,
  days,
  range,
  gridLabel,
  emptyText,
  dayAnnounceKey,
  showDayHeaders = true,
  dayLayout,
}: CalendarDayListProps) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const tabBarInset = useTabBarInset();
  const { hidden: hiddenCalendars } = useCalendarVisibility();

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [taskLists, setTaskLists] = useState<TaskList[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [sections, setSections] = useState<Section[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // The synced "show effort as tile size" pref (default on). Hydrated on mount
  // and re-read on focus so a Settings toggle / peer sync reflects without a
  // restart. Purely visual — the SR effort suffix is always appended below.
  const [effortSizing, setEffortSizing] = useState(true);
  useEffect(() => {
    const read = () =>
      void readTaskBehaviour().then((b) => setEffortSizing(b.visualEffortSizing));
    read();
    const unsubscribe = navigation.addListener('focus', read);
    return unsubscribe;
  }, [navigation]);

  const tr = useCallback(
    (key: string, vars?: Record<string, unknown>): string => t(key, vars) as string,
    [t],
  );
  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const dayKeys = useMemo(() => days.map(localDateKey), [days]);

  // Re-arm the grid auto-scroll whenever the visible day window changes (the
  // single-day grid caller swaps `days` on prev/next/jump). Clearing the guard
  // and the stale measured offsets lets the next layout pass scroll the new day
  // to its first event; without this, navigating to a new afternoon-only day
  // would stay parked at the previous day's offset. Grid-only state, but cheap
  // and harmless for the other callers.
  const dayWindowKey = dayKeys.join('|');
  useEffect(() => {
    scrolledDayKeyRef.current = null;
    daySectionYRef.current = null;
    gridRowYRef.current = null;
  }, [dayWindowKey]);

  const calendarsById = useMemo(
    () => new Map(calendars.map((c) => [c.id, c])),
    [calendars],
  );
  const labelsById = useMemo(
    () => new Map(colorLabels.map((l) => [l.id, l])),
    [colorLabels],
  );
  const listsById = useMemo(
    () => new Map(taskLists.map((l) => [l.id, l])),
    [taskLists],
  );
  const sectionColorById = useMemo(
    () => sectionColorMap(sections, labelsById),
    [sections, labelsById],
  );
  const readOnlyIds = useMemo(
    () => new Set(calendars.filter((c) => c.read_only).map((c) => c.id)),
    [calendars],
  );

  const fmtFullDate = useCallback(
    (d: Date) =>
      d.toLocaleDateString(i18n.language, {
        weekday: 'long',
        year: 'numeric',
        month: 'long',
        day: 'numeric',
      }),
    [i18n.language],
  );
  const fmtDateOnly = useMemo(() => {
    const f = new Intl.DateTimeFormat(i18n.language, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
    return (iso: string) => f.format(new Date(iso));
  }, [i18n.language]);
  const fmtTime = useCallback(
    (d: Date) =>
      d.toLocaleTimeString(i18n.language, { hour: '2-digit', minute: '2-digit' }),
    [i18n.language],
  );

  // Day's local midnight + `min` minutes, as a Date for `fmtTime`. EDGE: a
  // clamped end of 1440 (a tail running to the next midnight) has no same-day
  // Date — formatting midnight + 1440min would roll to "00:00" of day+1 and read
  // as "00:00", so eventTimeLabel special-cases 1440 → "24:00".
  const dayMinuteDate = useCallback((day: Date, min: number): Date => {
    const d = new Date(day);
    d.setHours(0, 0, 0, 0);
    d.setMinutes(min);
    return d;
  }, []);

  // The per-day time string for a TIMED event on `day`. When the event spans
  // midnight (multi-day), the time is the portion CLAMPED to `day` — so day N
  // reads "23:00–24:00" and day N+1 reads "00:00–01:00", matching what's drawn,
  // instead of the absolute instants (which would mislead a screen reader into
  // "23:00–01:00" on the tail day). A single-day event (multiDayInfo null) keeps
  // the absolute start/end. The 1440-minute end is shown as "24:00", not the
  // "00:00" a midnight Date would format to.
  const eventTimeLabel = useCallback(
    (ev: CalendarEvent, day?: Date): string => {
      if (ev.all_day) return t('views.allDay');
      const start = new Date(ev.start);
      const end = new Date(ev.end);
      if (day && multiDayInfo(ev, day)) {
        const sp = eventSpanForDay(start, end, day);
        const startStr = fmtTime(dayMinuteDate(day, sp.startMin));
        const endStr =
          sp.endMin === 1440 ? '24:00' : fmtTime(dayMinuteDate(day, sp.endMin));
        return `${startStr}–${endStr}`;
      }
      return `${fmtTime(start)}–${fmtTime(end)}`;
    },
    [dayMinuteDate, fmtTime, t],
  );

  // A request-epoch guard: the latest load wins. Changing the day window (e.g.
  // the week-start pref resolving async, or a prev/next step) recomputes `range`
  // and re-fires load while an earlier fetch may still be in flight; without
  // this, a slow earlier resolution could overwrite the newer window's data and
  // leave events mismatched against the day headers (derived from `days`).
  const reqToken = useRef(0);

  // ── Grid auto-scroll (dayLayout='grid' only; sighted/low-vision nicety) ──────
  // The grid renders a fixed 24h-tall canvas (CANVAS_PX) inside this ScrollView
  // with no initial offset, so an afternoon-only day would open showing an empty
  // 00:00–morning band. After layout we scroll ONCE per day so the first timed
  // slot (or, on today, the current hour) sits near the top. This is purely
  // visual: it touches no accessibilityLabel/role/action/tap handler, and only
  // the GRID path calls it — list/linear modes stack from the top already.
  const scrollRef = useRef<ScrollView>(null);
  // Measured y of the grid's day section and of its hour-grid row within that
  // section, captured via onLayout. Both are needed to locate the canvas top in
  // the scroll content. `null` until measured — if either is unmeasured we do
  // nothing (stay at the top) rather than scroll to a wrong place.
  const daySectionYRef = useRef<number | null>(null);
  const gridRowYRef = useRef<number | null>(null);
  // Guards one scroll per day so we don't fight the user's manual scroll on
  // every re-render. Reset (below) whenever the visible day key changes.
  const scrolledDayKeyRef = useRef<string | null>(null);

  // Try to scroll the grid so `b`'s earliest timed slot (or the current hour on
  // today) is near the top. No-ops unless: grid mode, both layout offsets are
  // measured, and we haven't already scrolled for THIS day. Never throws — any
  // missing measurement just leaves the scroll at the top.
  const maybeScrollGrid = useCallback(
    (b: DayBucket, earliestSlotTopPx: number | null) => {
      if (dayLayout !== 'grid') return;
      if (scrolledDayKeyRef.current === b.key) return;
      const sectionY = daySectionYRef.current;
      const rowY = gridRowYRef.current;
      if (sectionY == null || rowY == null) return; // unmeasured → stay at top
      // The canvas top within the scroll content: the day section's y plus the
      // grid row's y within that section (the all-day band sits above the row).
      const canvasY = sectionY + rowY;
      const today = localDateKey(new Date()) === b.key;
      // Today → current-hour offset; otherwise the earliest timed slot. If the
      // day has no timed items (earliestSlotTopPx == null) and it isn't today,
      // leave it at the top — all-day/untimed items live there.
      const withinCanvas = today
        ? (new Date().getHours() / 24) * CANVAS_PX
        : earliestSlotTopPx;
      if (withinCanvas == null) {
        // Mark the day handled so a no-timed-items day doesn't re-check forever.
        scrolledDayKeyRef.current = b.key;
        return;
      }
      const target = Math.max(0, canvasY + withinCanvas - GRID_SCROLL_PAD_PX);
      scrolledDayKeyRef.current = b.key;
      scrollRef.current?.scrollTo({ y: target, animated: false });
    },
    [dayLayout],
  );

  const load = useCallback(async () => {
    const token = (reqToken.current += 1);
    setLoading(true);
    setError(null);
    try {
      // listCalendars also primes the Host's route map (getEvents routes by
      // calendar id), so it must resolve before the per-calendar fetch. Palette,
      // lists are best-effort — a failure just drops the colour/task overlay.
      const [cals, labels, lists] = await Promise.all([
        listCalendars(),
        listColorLabels().catch(() => [] as ColorLabel[]),
        listTaskLists().catch(() => [] as TaskList[]),
      ]);
      const startIso = range.start.toISOString();
      const endIso = range.end.toISOString();
      const [perCalendar, perList, perListSections] = await Promise.all([
        Promise.all(
          cals.map((c) =>
            getEvents({ calendar_id: c.id, start: startIso, end: endIso }).catch(() => []),
          ),
        ),
        Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[]))),
        Promise.all(lists.map((l) => getSections(l.id).catch(() => [] as Section[]))),
      ]);
      // A newer load superseded this one — drop these stale results.
      if (reqToken.current !== token) return;
      setCalendars(cals);
      setColorLabels(labels);
      setTaskLists(lists);
      // Expand recurring series across the whole window so an event recurring
      // mid-window isn't invisible after its first occurrence (rrule + EXDATE).
      setEvents(expandAll(perCalendar.flat(), { start: range.start, end: range.end }));
      setTasks(perList.flat());
      setSections(perListSections.flat());
    } catch (err) {
      if (reqToken.current !== token) return;
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      if (reqToken.current === token) setLoading(false);
    }
  }, [announce, range, t]);

  // Reload when the window changes or the screen regains focus (after an editor).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

  // Live-update while focused: this view loads BOTH events and tasks per day, so
  // re-read on either an external calendar- or task-cache refresh (the root
  // observer already announced it politely; same `load` covers both).
  useCacheReload('calendar', load);
  useCacheReload('tasks', load);

  // Per-list "show completed in calendar" opt-in (default hide). Shared store, so
  // a toggle on the Lists screen reflects here on the next render.
  const { shouldShow: showCompletedForList } = useTaskListShowCompleted();
  const currentUserByList = useCurrentUserByList(tasks);
  // Hide tasks assigned to a concrete OTHER user from MY calendar (mine +
  // unassigned stay) — the day-start review's ownership filter (DESIGN §9.7).
  const meFor = useCallback(
    (listId: string) => currentUserByList[listId] ?? null,
    [currentUserByList],
  );

  // Bucket each day's events + tasks. Completed tasks stay hidden by default;
  // per list, the "show completed in calendar" opt-in (toggled on the Lists
  // screen, shared with the desktop's `tasks.showCompletedInCalendar` pref)
  // keeps them visible.
  // Calendars the user hid (the Calendars-screen toggles) drop out of the view.
  const visibleEvents = useMemo(
    () => events.filter((ev) => !hiddenCalendars.has(ev.calendar_id)),
    [events, hiddenCalendars],
  );
  const buckets = useMemo<DayBucket[]>(() => {
    return days.map((date, i) => {
      const key = dayKeys[i];
      const allDay = visibleEvents.filter(
        (ev) => ev.all_day && daysCoveredKeys(ev).includes(key),
      );
      const timedEvents = visibleEvents.filter(
        // daysCoveredKeys spreads a timed event across midnight too, so a
        // 23:00→01:00 meeting buckets onto both days (mergeDayItems +
        // eventSpanForDay clamp each day's portion).
        (ev) => !ev.all_day && daysCoveredKeys(ev).includes(key),
      );
      const dayTasks = filterTasksOnDay(tasks, key, showCompletedForList, meFor);
      const { timed, untimed } = mergeDayItems(
        timedEvents,
        dayTasks,
        key,
        (ev) => new Date(ev.start).getTime(),
      );
      return {
        key,
        date,
        allDay,
        timed,
        untimed,
        count: allDay.length + timed.length + untimed.length,
      };
    });
  }, [days, dayKeys, visibleEvents, tasks, showCompletedForList, meFor]);

  const totalItems = useMemo(
    () => buckets.reduce((sum, b) => sum + b.count, 0),
    [buckets],
  );

  const editEvent = useCallback(
    (ev: CalendarEvent) =>
      navigation.navigate('EventEditor', {
        eventId: seriesIdOf(ev),
        calendarId: ev.calendar_id,
        occurrence: occurrenceIsoOf(ev),
      }),
    [navigation],
  );

  // Move / copy to another calendar — pass the full (possibly expanded) row so
  // the modal can offer the occurrence-vs-series scope.
  const moveCopyEvent = useCallback(
    (ev: CalendarEvent) => navigation.navigate('MoveCopy', { kind: 'event', event: ev }),
    [navigation],
  );

  const removeEvent = useCallback(
    (ev: CalendarEvent) =>
      confirmDeleteEvent(
        ev,
        t,
        (message) => {
          announce(message);
          void load();
        },
        (message) => {
          setError(message);
          announce(t('mobile.error', { message }));
        },
      ),
    [announce, load, t],
  );

  const openTask = useCallback(
    (task: Task) =>
      navigation.navigate('TaskEditor', { taskId: task.id, listId: task.list_id }),
    [navigation],
  );

  const moveCopyTask = useCallback(
    (task: Task) =>
      navigation.navigate('MoveCopy', {
        kind: 'task',
        taskId: task.id,
        listId: task.list_id,
      }),
    [navigation],
  );

  // Per-day "+ new event": seed a new event on that day (first writable
  // calendar) — the mobile twin of the desktop's day-anchored create.
  const firstWritableCalendarId = useMemo(
    () =>
      calendars.find((c) => !c.read_only && !hiddenCalendars.has(c.id))?.id ??
      calendars.find((c) => !c.read_only)?.id ??
      null,
    [calendars, hiddenCalendars],
  );
  const addEventOnDay = useCallback(
    (dayKey: string) => {
      if (firstWritableCalendarId == null) return;
      // → the event quick-add (expands to the full editor via "More details …").
      navigation.navigate('QuickAddEvent', {
        calendarId: firstWritableCalendarId,
        anchor: dayKey,
      });
    },
    [firstWritableCalendarId, navigation],
  );

  // Day-anchored task create — seed the new task's scheduled day from the tapped
  // calendar day (the task twin of addEventOnDay). Lands in the first writable
  // task list; the editor's list picker can move it.
  const firstWritableTaskListId = useMemo(
    () => taskLists.find((l) => !l.read_only)?.id ?? null,
    [taskLists],
  );
  const addTaskOnDay = useCallback(
    (dayKey: string) => {
      // → the task quick-add, anchored to the tapped day (expands to the full
      //   editor via "More details …"). The quick-add picks the list itself
      //   (last-used), mirroring the desktop day-activation create flow.
      navigation.navigate('QuickAdd', { initialScheduledDate: dayKey });
    },
    [navigation],
  );

  // Check off a task via the shared toggle path (honours the synced
  // task-behaviour knobs), then reload. Like the other calendar screens, focus
  // is not forcibly restored across the reload.
  const toggleTask = useCallback(
    async (task: Task) => {
      try {
        const next = await applyTaskToggle(task, listsById.get(task.list_id), tasks);
        if (next == null) return;
        announce(statusAnnounce(t, next, task.title));
        await load();
      } catch (err) {
        announce(t('mobile.error', { message: errorMessage(err) }));
      }
    },
    [announce, listsById, load, t, tasks],
  );

  const removeTask = useCallback(
    (task: Task) => {
      Alert.alert(
        t('dialogs.confirm.deleteTaskTitle'),
        t('dialogs.confirm.deleteTaskMessage', { title: task.title }),
        [
          { text: t('dialogs.confirm.cancel'), style: 'cancel' },
          {
            text: t('mobile.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                try {
                  await apiDeleteTask(task.id, task.list_id);
                  announce(t('mobile.deleted', { title: task.title }));
                  await load();
                } catch (err) {
                  announce(t('mobile.error', { message: errorMessage(err) }));
                }
              })();
            },
          },
        ],
      );
    },
    [announce, load, t],
  );

  // ── Accessible labels ──────────────────────────────────────────────────────

  const eventLabel = useCallback(
    (ev: CalendarEvent, day: Date, span: MultiDayInfo | null): string => {
      let label = t('views.week.eventLabel', {
        title: ev.title,
        // Per-day clamped time on a multi-day (cross-midnight) timed event, so a
        // screen reader hears the portion that actually falls on THIS day
        // (eventTimeLabel handles the clamp + the "24:00" tail edge).
        time: eventTimeLabel(ev, day),
        calendar: calendarsById.get(ev.calendar_id)?.name ?? '—',
      });
      if (span) {
        label += t('views.multiDaySuffix', { day: span.dayIndex, total: span.totalDays });
      }
      const colour = resolveEventColor(ev, calendarsById, labelsById);
      if (colour.labelName) {
        label += t('mobile.colorLabelSuffix', { name: colour.labelName });
      }
      return label;
    },
    [calendarsById, eventTimeLabel, labelsById, t],
  );

  const taskLabel = useCallback(
    (task: Task, key: string, colourName: string | null): string => {
      const time = taskTimeOnDay(task, key);
      const common = {
        title: task.title,
        state: t(statusI18nKey(task.status)),
        priority: prioritySuffix(tr, task.priority),
        progress: subtaskProgressSuffix(tr, task.id, tasks),
        assignee: assigneeSuffix(tr, task.assignees),
      };
      let label: string;
      if (time) {
        label = t('views.week.taskChipTimed', {
          ...common,
          time: fmtTime(buildTimeDate(key, time)),
        });
      } else if (task.deadline_date) {
        // Any untimed task with a deadline announces "fällig bis …": a pure
        // deadline-only marker on its due day, OR a scheduled task carrying its
        // deadline on its plan row (the deadline-day duplicate is suppressed in
        // filterTasksOnDay, so the plan row is where the due date is spoken).
        label = t('views.week.taskChipBy', {
          ...common,
          deadline: fmtDateOnly(task.deadline_date),
        });
      } else {
        label = t('views.week.taskChip', common);
      }
      if (colourName) {
        label += t('mobile.colorLabelSuffix', { name: colourName });
      }
      // Effort suffix appended unconditionally (regardless of the visual-sizing
      // toggle) so a screen reader always hears it; '' for medium.
      return label + effortSuffix(tr, task.effort) + subtaskParentSuffix(tr, task, tasks);
    },
    [fmtDateOnly, fmtTime, t, tasks, tr],
  );

  // `slot` (grid mode only) absolutely positions the row inside the 24h canvas
  // and switches the visible chip to title-only (the time is read off the ruler;
  // it stays in the accessibilityLabel below, unchanged). `extraStyle` (compact
  // LIST mode only) carries the duration-proportional STRICT height (+ overflow:
  // 'hidden') so the coloured event block fills that height and a long wrapping
  // title clips instead of inflating a short event past a long one.
  const renderEventRow = (
    ev: CalendarEvent,
    day: Date,
    span: MultiDayInfo | null,
    slot?: PositionedSpan,
    extraStyle?: StyleProp<ViewStyle>,
  ) => {
    const rowKey = `e-${ev.id}@${localDateKey(day)}`;
    const hex = resolveEventColor(ev, calendarsById, labelsById).hex;
    const grid = slot != null;
    const dot =
      hex != null ? (
        <View
          accessible={false}
          importantForAccessibility="no"
          style={[styles.colorDot, { backgroundColor: hex }]}
        />
      ) : null;
    const badge = span
      ? ` ${t('views.multiDayCompact', { day: span.dayIndex, total: span.totalDays })}`
      : '';
    if (readOnlyIds.has(ev.calendar_id)) {
      return (
        <View
          key={rowKey}
          accessible
          accessibilityRole="text"
          accessibilityLabel={eventLabel(ev, day, span)}
          style={grid ? [styles.gridChip, slotStyle(slot)] : [styles.row, extraStyle]}
        >
          {dot}
          <View style={styles.rowText}>
            <Text
              style={styles.itemTitle}
              importantForAccessibility="no"
              numberOfLines={grid ? 1 : undefined}
            >
              {ev.title}
              {badge}
            </Text>
            {!grid && (
              <Text style={styles.itemMeta} importantForAccessibility="no">
                {eventTimeLabel(ev, day)}
              </Text>
            )}
          </View>
        </View>
      );
    }
    return (
      <View
        key={rowKey}
        accessible
        accessibilityRole="button"
        accessibilityLabel={eventLabel(ev, day, span)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={[
          { name: 'activate', label: t('mobile.editTaskLabel') },
          { name: 'moveCopy', label: t('mobile.moveCopy') },
          { name: 'delete', label: t('dialogs.event.delete') },
        ]}
        onAccessibilityAction={(e) => {
          if (e.nativeEvent.actionName === 'delete') removeEvent(ev);
          else if (e.nativeEvent.actionName === 'moveCopy') moveCopyEvent(ev);
          else editEvent(ev);
        }}
        style={grid ? [styles.gridChip, slotStyle(slot)] : [styles.row, extraStyle]}
      >
        {dot}
        <Pressable accessible={false} onPress={() => editEvent(ev)} style={styles.rowText}>
          <Text
            style={styles.itemTitle}
            importantForAccessibility="no"
            numberOfLines={grid ? 1 : undefined}
          >
            {ev.title}
            {badge}
          </Text>
          {!grid && (
            <Text style={styles.itemMeta} importantForAccessibility="no">
              {eventTimeLabel(ev, day)}
            </Text>
          )}
        </Pressable>
        {/* In the compact grid chip the inline delete button would crowd out
            the title; SR users keep the row's "delete" custom action. */}
        {!grid && (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${t('dialogs.event.delete')}: ${ev.title}`}
            onPress={() => removeEvent(ev)}
            style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
          >
            <Text style={styles.deleteButtonText}>{t('dialogs.event.delete')}</Text>
          </Pressable>
        )}
      </View>
    );
  };

  const renderTaskRow = (task: Task, key: string, slot?: PositionedSpan) => {
    const done = task.status === 'completed';
    const grid = slot != null;
    const resolved = resolveTaskColor(task, listsById, labelsById, sectionColorById);
    const hex = resolved.hex;
    // Visual tile-size by effort (sighted users), only when the synced pref is
    // on; medium = neutral base row height. Purely cosmetic — the effort is
    // always in the row's accessibilityLabel via effortSuffix.
    // In the LINEAR list (and the compact list-view) this is a row style
    // (rowEffortSmall/Large). In the hour-GRID a chip is absolutely positioned
    // by time, so we can't restyle its row height freely; instead we RAISE its
    // slot FLOOR per effort (small/neutral/large) via slotStyle's `floorPx`
    // below — the slot's TOP (its time position) is preserved, so higher-effort
    // tasks read as taller blocks without drifting off their time.
    const effortStyle =
      !grid && effortSizing
        ? effortSizeModifier(task.effort) === 'small'
          ? styles.rowEffortSmall
          : effortSizeModifier(task.effort) === 'large'
            ? styles.rowEffortLarge
            : null
        : null;
    // Day-aware visible meta (the row's reason for being on THIS day): its time
    // if timed here, else a "due"/"planned" marker for this day. (Task-level
    // describeDue would show the scheduled day on a deadline-day row.)
    const time = taskTimeOnDay(task, key);
    let meta = time
      ? fmtTime(buildTimeDate(key, time))
      : isDeadlineChip(task, key)
        ? t('views.tasks.dueDeadline', { date: fmtDateOnly(key) })
        : t('views.tasks.dueScheduled', { date: fmtDateOnly(key) });
    // A scheduled task now carries its deadline on its plan row (no separate
    // deadline-day row), so surface the due date visibly alongside the meta.
    if (!isDeadlineChip(task, key) && task.deadline_date) {
      meta += ` · ${t('views.week.taskChipDeadlineBadge', {
        deadline: fmtDateOnly(task.deadline_date),
      })}`;
    }
    return (
      <View
        key={`t-${task.id}@${key}`}
        accessible
        accessibilityRole="button"
        accessibilityLabel={taskLabel(task, key, resolved.labelName)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={[
          { name: 'toggle', label: done ? t('mobile.reopen') : t('mobile.complete') },
          { name: 'edit', label: t('mobile.rename') },
          { name: 'moveCopy', label: t('mobile.moveCopy') },
          { name: 'delete', label: t('mobile.delete') },
        ]}
        onAccessibilityAction={(e) => {
          const name = e.nativeEvent.actionName;
          if (name === 'toggle') void toggleTask(task);
          else if (name === 'delete') removeTask(task);
          else if (name === 'moveCopy') moveCopyTask(task);
          else openTask(task);
        }}
        style={
          grid
            ? [
                styles.gridChip,
                slotStyle(slot, effortSizing ? GRID_TASK_EFFORT_PX[task.effort] : MIN_SLOT_PX),
              ]
            : [styles.row, effortStyle]
        }
      >
        {/* Sighted tap target to complete/reopen the task; the row otherwise
            opens the task on tap, so the marker needs its own Pressable. SR
            users use the row's "toggle" custom action, so this stays out of the
            accessibility tree. */}
        <Pressable
          accessible={false}
          importantForAccessibility="no"
          onPress={() => void toggleTask(task)}
          hitSlop={10}
          style={({ pressed }) => [styles.taskCheckButton, pressed && styles.pressed]}
        >
          <Text style={styles.taskCheck} importantForAccessibility="no">
            {statusMarker(task.status)}
          </Text>
        </Pressable>
        {hex != null && (
          <View
            accessible={false}
            importantForAccessibility="no"
            style={[styles.colorDot, { backgroundColor: hex }]}
          />
        )}
        <Pressable accessible={false} onPress={() => openTask(task)} style={styles.rowText}>
          <Text
            style={[styles.itemTitle, done && styles.itemTitleDone]}
            importantForAccessibility="no"
            numberOfLines={grid ? 1 : undefined}
          >
            {task.parent_id ? '↳ ' : ''}
            {task.title}
          </Text>
          {!grid && (
            <Text style={styles.itemMeta} importantForAccessibility="no">
              {meta}
            </Text>
          )}
        </Pressable>
      </View>
    );
  };

  // Hour-grid placement for one day's timed items (single-day only). Each timed
  // item gets an absolute slot keyed by its index in `b.timed`; source order is
  // untouched. Every timed task ALWAYS gets a slot (unparseable time → midnight)
  // so nothing flows static and corrupts the positioned canvas. Mirrors the
  // desktop DayView's `slotByIdx`.
  const computeSlots = (b: DayBucket): Map<number, PositionedSpan> => {
    const map = new Map<number, PositionedSpan>();
    const spans: TimedSpan[] = [];
    const slotIdxs: number[] = [];
    b.timed.forEach((item, idx) => {
      let s: TimedSpan | null = null;
      if (item.kind === 'event') {
        // All-day events never reach b.timed (they're in b.allDay), but guard
        // anyway — an all-day chip has no meaningful hour placement.
        if (!item.event.all_day) {
          s = eventSpanForDay(new Date(item.event.start), new Date(item.event.end), b.date);
        }
      } else {
        // A timed task is a zero-duration point; an unparseable time falls back
        // to midnight so it ALWAYS gets a slot.
        const m = minutesFromMidnight(taskTimeOnDay(item.task, b.key) ?? '');
        s = { startMin: m ?? 0, endMin: m ?? 0 };
      }
      if (s) {
        spans.push(s);
        slotIdxs.push(idx);
      }
    });
    const positions = layoutDayColumn(spans);
    slotIdxs.forEach((idx, k) => map.set(idx, positions[k]));
    return map;
  };

  // The day's +event / +task create buttons — shared by the list and grid
  // layouts (in the grid they sit below the canvas, still reachable).
  const renderDayCreateButtons = (b: DayBucket): ReactNode => (
    <>
      {firstWritableCalendarId != null && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('toolbar.newEvent')}, ${fmtFullDate(b.date)}`}
          onPress={() => addEventOnDay(b.key)}
          style={({ pressed }) => [styles.newEventButton, pressed && styles.pressed]}
        >
          <Text style={styles.newEventText}>{t('toolbar.newEvent')}</Text>
        </Pressable>
      )}
      {firstWritableTaskListId != null && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('toolbar.newTask')}, ${fmtFullDate(b.date)}`}
          onPress={() => addTaskOnDay(b.key)}
          style={({ pressed }) => [styles.newEventButton, pressed && styles.pressed]}
        >
          <Text style={styles.newEventText}>{t('toolbar.newTask')}</Text>
        </Pressable>
      )}
    </>
  );

  // Single-day hour-grid: all-day events, then the 24h canvas (a leading
  // hour-ruler 00–23 beside the positioned day column), then a compact band of
  // UNTIMED tasks BELOW the canvas (per Toni: the task band stays under the grid,
  // not above). Reading/source order: all-day → timed(canvas) → untimed → create
  // buttons (events first, then tasks). Each untimed row uses the SAME
  // renderTaskRow (no slot), so its accessibilityRole/Label/actions/effort sizing
  // + tap handlers are unchanged. The grid is visual only; within the canvas
  // timed order is preserved chronologically.
  const renderDayGrid = (b: DayBucket): ReactNode => {
    const slots = computeSlots(b);
    // The earliest timed slot's top in px (the highest chip on the canvas), or
    // null when the day has no timed items. Drives the auto-scroll target for a
    // non-today day; `slots` already reflects the overlap layout, and topFraction
    // is the chip's start position, so min(topFraction)*CANVAS_PX is the top of
    // the first event/timed-task of the day.
    const slotTops = Array.from(slots.values(), (p) => p.topFraction * CANVAS_PX);
    const earliestSlotTopPx = slotTops.length > 0 ? Math.min(...slotTops) : null;
    return (
      <View
        key={b.key}
        style={styles.daySection}
        // y of this section within the scroll content (the all-day band sits at
        // its top, the hour-grid row below). Combined with the grid row's y to
        // locate the canvas top, then we try the one-per-day scroll.
        onLayout={(e) => {
          daySectionYRef.current = e.nativeEvent.layout.y;
          maybeScrollGrid(b, earliestSlotTopPx);
        }}
      >
        {b.allDay.map((ev) => renderEventRow(ev, b.date, multiDayInfo(ev, b.date)))}
        <View
          style={styles.gridRow}
          // y of the hour-grid row within the day section (below the all-day
          // band). daySectionY + this = the canvas top in the scroll content.
          onLayout={(e) => {
            gridRowYRef.current = e.nativeEvent.layout.y;
            maybeScrollGrid(b, earliestSlotTopPx);
          }}
        >
          {/* Hour ruler — the hour numbers (00–23), read off the grid instead of
              the chips. Decorative: the time stays in each row's
              accessibilityLabel, so it's hidden from the screen reader. */}
          <View
            style={styles.ruler}
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
          >
            {Array.from({ length: 24 }, (_, h) => (
              <Text key={h} style={[styles.rulerHour, { top: (h / 24) * CANVAS_PX }]}>
                {String(h).padStart(2, '0')}
              </Text>
            ))}
          </View>
          {/* The positioned 24h day column. Each timed chip is absolutely placed
              by start+duration; faint hour gridlines read the column as a grid. */}
          <View style={styles.canvas}>
            {Array.from({ length: 24 }, (_, h) => (
              <View
                key={h}
                accessibilityElementsHidden
                importantForAccessibility="no-hide-descendants"
                style={[styles.gridLine, { top: (h / 24) * CANVAS_PX }]}
              />
            ))}
            {b.timed.map((item, idx) =>
              item.kind === 'event'
                ? renderEventRow(item.event, b.date, multiDayInfo(item.event, b.date), slots.get(idx))
                : renderTaskRow(item.task, b.key, slots.get(idx)),
            )}
          </View>
        </View>
        {/* Compact band of untimed tasks, BELOW the canvas (per Toni: the task
            band stays under the grid, not above it). A short heading then the
            same renderTaskRow rows (no slot → unchanged a11y + effort sizing +
            tap handlers); the band container is a little tighter. Reading order
            is all-day → timed(canvas) → untimed → create buttons. */}
        {b.untimed.length > 0 && (
          <View style={styles.taskBand}>
            <Text accessibilityRole="header" style={styles.taskBandHeading}>
              {t('views.day.tasksHeading')}
            </Text>
            {b.untimed.map((task) => renderTaskRow(task, b.key))}
          </View>
        )}
        {renderDayCreateButtons(b)}
      </View>
    );
  };

  // Single-day COMPACT LIST (dayLayout='list'): the lighter alternative to the
  // hour-grid. Same reading order (all-day → timed → untimed → create buttons)
  // and the SAME row renderers (renderEventRow/renderTaskRow without a slot), so
  // every row's accessibilityRole/Label/actions/tap handlers and source order
  // are identical to the linear list. The only visual difference: a timed EVENT
  // block's own STRICT height reflects its DURATION (eventBlockFactor) so the
  // coloured block grows taller for a longer meeting (a long title clips rather
  // than inflating it), and a timed TASK keeps its effort sizing (renderTaskRow's
  // `!grid` path).
  const renderDayList = (b: DayBucket): ReactNode => (
    <View key={b.key} style={styles.daySection}>
      {b.allDay.map((ev) => renderEventRow(ev, b.date, multiDayInfo(ev, b.date)))}
      {b.timed.map((item) => {
        if (item.kind === 'task') return renderTaskRow(item.task, b.key);
        const ev = item.event;
        const height = Math.round(
          eventBlockFactor(eventDurationMinForDay(new Date(ev.start), new Date(ev.end), b.date)) *
            LIST_EVENT_BASE_PX,
        );
        // A STRICT duration-driven height (not a min-height floor) on the event
        // ROW's own style, with overflow:'hidden', so the coloured block both
        // fills the reserved height AND can never be inflated past its duration
        // by a long wrapping title — the title clips vertically while the row's
        // horizontal children (colour dot, time/meta, delete affordance) stay
        // laid out and tappable. Mirrors the desktop WeekView list chip. The row
        // keeps its own role/label/actions unchanged; the full title lives in the
        // accessibilityLabel and tapping the row opens the editor (no clipping
        // for SR users).
        return renderEventRow(ev, b.date, multiDayInfo(ev, b.date), undefined, { height, overflow: 'hidden' });
      })}
      {b.untimed.map((task) => renderTaskRow(task, b.key))}
      {renderDayCreateButtons(b)}
    </View>
  );

  // The error line rides above whatever else renders (the list still shows when
  // a reload after a mutation fails but stale data remains).
  return (
    <>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('views.loading')}>
          {t('views.loading')}
        </Text>
      ) : totalItems === 0 ? (
        <Text style={styles.muted} accessibilityRole="text">
          {emptyText}
        </Text>
      ) : (
        <ScrollView
          ref={scrollRef}
          accessibilityRole="list"
          accessibilityLabel={gridLabel}
          style={styles.scroll}
          contentContainerStyle={[styles.list, { paddingBottom: tabBarInset }]}
          keyboardShouldPersistTaps="handled"
        >
          {buckets.map((b) => {
            // Single-day paths. Gated on the explicit `dayLayout` prop, which
            // ONLY the single-day caller (EventsScreen) passes — multi-day
            // Week/Month/Agenda pass nothing and fall through to the linear list
            // below, unchanged.
            if (dayLayout === 'grid') return renderDayGrid(b);
            if (dayLayout === 'list') return renderDayList(b);
            const rows: ReactNode[] = [];
            if (showDayHeaders) {
              rows.push(
                <Text
                  key={`h-${b.key}`}
                  accessibilityRole="header"
                  accessibilityLabel={t(dayAnnounceKey, {
                    day: fmtFullDate(b.date),
                    count: b.count,
                  })}
                  style={styles.dayHeader}
                >
                  {fmtFullDate(b.date)}
                </Text>,
              );
            }
            for (const ev of b.allDay) {
              rows.push(renderEventRow(ev, b.date, multiDayInfo(ev, b.date)));
            }
            for (const item of b.timed) {
              rows.push(
                item.kind === 'event'
                  ? renderEventRow(item.event, b.date, multiDayInfo(item.event, b.date))
                  : renderTaskRow(item.task, b.key),
              );
            }
            for (const task of b.untimed) {
              rows.push(renderTaskRow(task, b.key));
            }
            return (
              <View key={b.key} style={styles.daySection}>
                {rows}
                {renderDayCreateButtons(b)}
              </View>
            );
          })}
        </ScrollView>
      )}
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    scroll: { flex: 1 },
    list: { gap: 8, padding: 16 },
    daySection: { gap: 8 },
    newEventButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    newEventText: { fontSize: 15, fontWeight: '600', color: c.link },
    dayHeader: {
      fontSize: 15,
      fontWeight: '700',
      color: c.textLabel,
      marginTop: 8,
      marginBottom: 2,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    // Effort-driven chip height (gated on the visualEffortSizing pref). Medium
    // uses the base `row` size; small is more compact, large a bit taller.
    rowEffortSmall: { paddingVertical: 8, minHeight: 0 },
    rowEffortLarge: { paddingVertical: 24, minHeight: 96 },
    rowText: { flex: 1, gap: 2 },
    taskCheckButton: { borderRadius: 8, padding: 4 },
    taskCheck: { fontSize: 20, width: 26, textAlign: 'center', color: c.textPrimary },
    colorDot: {
      width: 12,
      height: 12,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: c.borderOverlay,
    },
    itemTitle: { fontSize: 18, fontWeight: '600', color: c.textPrimary },
    itemTitleDone: { textDecorationLine: 'line-through', color: c.textSecondary },
    itemMeta: { fontSize: 14, color: c.textSecondary },
    // ── Single-day untimed-task band (dayLayout='grid') ──────────────────────
    // A compact band of the day's untimed tasks, sitting ABOVE the 24h canvas
    // (between the all-day rows and the hour-grid). A little tighter than the
    // canvas area (smaller gap) so it reads as a top band, while each task row
    // inside keeps its own full padding / tap target (renderTaskRow's `row`).
    taskBand: { gap: 4 },
    taskBandHeading: {
      fontSize: 13,
      fontWeight: '700',
      color: c.textLabel,
      marginBottom: 2,
    },
    // ── Single-day hour-grid (dayLayout='grid') ──────────────────────────────
    // A horizontal [ruler | 24h canvas] row; the canvas is a 24h-tall positioned
    // column. Ruler numbers + chips line up because both are offset from the
    // SAME top by the same per-hour fraction (CANVAS_PX tall).
    gridRow: { flexDirection: 'row', alignItems: 'flex-start' },
    ruler: { width: RULER_PX, height: CANVAS_PX, position: 'relative' },
    rulerHour: {
      position: 'absolute',
      right: 6,
      fontSize: 12,
      color: c.textSecondary,
      // Nudge up so the number sits centred on its gridline.
      marginTop: -7,
    },
    canvas: { flex: 1, height: CANVAS_PX, position: 'relative' },
    gridLine: {
      position: 'absolute',
      left: 0,
      right: 0,
      height: 1,
      backgroundColor: c.border,
      opacity: 0.5,
    },
    // A timed chip inside the canvas: absolutely positioned (slotStyle supplies
    // top/height/left/width), compact, title-only. overflow:hidden clips a long
    // title to the slot height so it never spills over neighbours.
    gridChip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      paddingVertical: 4,
      paddingHorizontal: 8,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      overflow: 'hidden',
    },
    deleteButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    deleteButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
