import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  PixelRatio,
  Pressable,
  RefreshControl,
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
  PriorityScale,
  Section,
  Task,
  TaskList,
  TimedSpan,
} from '@aperio/shared';
import {
  eventInstanceKey,
  withoutDuplicateMeetings,
  collapseEventGroups,
  eventGroupMemberKey,
  groupBadge,
  findHealableMembers,
  findMeetingLinkPairs,
  findStaleSignatures,
  indexEventGroups,
  memberFromEvent,
  seriesIdOf,
  type CollapsedRow,
  type EventGroup,
} from '@aperio/shared';
import {
  assigneeSuffix,
  daysCoveredKeys,
  effortSizeModifier,
  effortSuffix,
  eventBlockFactor,
  eventSpanForDay,
  expandAll,
  expandScheduledRecurringTasks,
  filterTasksOnDay,
  isDeadlineChip,
  isRecurringProjection,
  layoutDayColumn,
  localDateKey,
  mergeDayItems,
  MINUTES_PER_DAY,
  minutesFromMidnight,
  multiDayInfo,
  prioritySuffix,
  recurringSeriesTaskId,
  sortSections,
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
import { joinAction, openConference } from '../intl/conferencing';
import { resolveEventColor } from '../intl/eventColor';
import { resolveTaskColor, sectionColorMap } from '../intl/taskColor';
import type { RootStackParamList } from '../navigation/types';
import {
  eventGroupsForEvents,
  groupEvents,
  groupSuggestionDeclines,
  healEventGroupMember,
  refreshEventGroupSignature,
} from '../api/eventGroups';
import { ActionsMenu, type MenuAction } from './ActionsMenu';
import { GroupSuggestionNotice } from './GroupSuggestionNotice';
import { useCacheReload } from '../state/cacheObserver';
import { hapticLoadBegin, hapticLoadEnd } from '../state/haptics';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { useCurrentUserByList } from '../state/currentUser';
import { confirmDeleteEvent } from '../state/eventDeleteScope';
import { confirmDeleteTask } from '../state/taskDeleteScope';
import { usePullRefresh } from '../state/usePullRefresh';
import { editEventWithScope } from '../state/eventEditScope';
import { priorityScaleFor, readTaskBehaviour } from '../state/taskBehaviour';
import { applyTaskToggle, statusAnnounce } from '../state/taskToggle';
import { useTaskListShowCompleted } from '../state/useTaskListShowCompleted';
import { useThemedStyles, type ThemeColors } from '../theme';
import { chrome } from '../theme/uiScale';

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
// which derives from HOUR_PX × the visible hours) grows proportionally with the
// text, keeping the single-line labels readable. This is the mobile twin of the
// desktop `--hour-px`→rem fix. Read once at module load (a font-scale change
// while the app runs is picked up on the next launch — RN reloads on most such
// changes).
const FONT_SCALE = PixelRatio.getFontScale();

/** Canvas pixels per hour → the canvas is HOUR_PX × (visible hours) tall. With
 *  the default full-day window the column is HOUR_PX*24; a narrower visible
 *  window (the synced `calendar.dayStartMin`/`dayEndMin` pref) shortens it
 *  proportionally so the grid spans only [dayStartMin, dayEndMin]. The mobile
 *  twin of the desktop `--hour-px` × `--day-hours` height. */
const HOUR_PX = Math.round(48 * FONT_SCALE);
/** Minimum rendered chip height so a short/zero-duration item stays legible. */
const MIN_SLOT_PX = Math.round(28 * FONT_SCALE);
/** Hour-ruler column width (carries the 00–23 numbers). */
const RULER_PX = Math.round(44 * FONT_SCALE);
/** Top padding (px) left above the auto-scroll target in GRID mode so the first
 *  event isn't flush to the very top edge — a little breathing room reads
 *  better. Purely visual; only the single-day grid auto-scroll uses it. */
const GRID_SCROLL_PAD_PX = 12;

/** Debounce (ms) for the day-list reload. Longer than a fast VoiceOver
 *  three-finger-swipe interval so rapid paging collapses into a single reload on
 *  the period the user settles on — instead of churning the accessibility tree on
 *  every step (which delayed the announcement and blocked the next swipe). */
const RELOAD_DEBOUNCE_MS = 300;

// ── Per-container retention (mirror of the desktop useEvents fix) ────────────
// Last successful batch per container, module-level so it survives remounts.
// A container whose read FAILS during a reload reuses its last batch instead
// of degrading to [] — one hiccuping backend must not shrink the aggregated
// day (and with it the entry count screen readers announce). Successful reads
// always replace the slice, so genuine deletions still propagate. Events are
// additionally keyed by the fetched range so no cross-range rows can leak —
// which also means paging accumulates one entry per calendar per visited
// range, so the events map is capped (oldest-inserted evicted; re-set keys
// are refreshed to the tail so active ranges stay resident). The per-list
// maps are naturally bounded by the list count.
const EVENTS_RETENTION_MAX_ENTRIES = 200;
const perCalendarEventsCache = new Map<string, CalendarEvent[]>();
const perListTasksCache = new Map<string, Task[]>();
const perListSectionsCache = new Map<string, Section[]>();
let lastKnownColorLabels: ColorLabel[] = [];
let lastKnownTaskLists: TaskList[] = [];

function retainEvents(key: string, batch: CalendarEvent[]): void {
  // Delete-then-set keeps Map insertion order ≈ recency, so eviction below
  // drops the least recently WRITTEN range first.
  perCalendarEventsCache.delete(key);
  perCalendarEventsCache.set(key, batch);
  while (perCalendarEventsCache.size > EVENTS_RETENTION_MAX_ENTRIES) {
    const oldest = perCalendarEventsCache.keys().next().value;
    if (oldest == null) break;
    perCalendarEventsCache.delete(oldest);
  }
}

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
 *  on-canvas. One step above the original mapping (tester feedback):
 *  small == MIN_SLOT_PX keeps the unmodified slot floor (NEUTRAL, matching
 *  desktop where small has no effort class), medium takes the former large
 *  height, large grew beyond it. */
const GRID_TASK_EFFORT_PX = {
  small: MIN_SLOT_PX,
  medium: Math.round(46 * FONT_SCALE),
  large: Math.round(72 * FONT_SCALE),
} as const;

/** Timed event's clamped duration in minutes on `day` (for the list-view block
 *  height). Reuses the shared eventSpanForDay so it matches the grid's duration
 *  math. */
function eventDurationMinForDay(start: Date, end: Date, day: Date): number {
  const span = eventSpanForDay(start, end, day);
  return span.endMin - span.startMin;
}

/** Absolute placement of a timed chip inside the (windowed) canvas (purely
 *  visual; source order is unchanged). top/height by start+duration, left/width
 *  by the overlap column. The fractions are WINDOW-RELATIVE (the shared
 *  `layoutDayColumn` already classified the item 'in' and clamped it to the
 *  visible window), so they're multiplied by `canvasPx` — the WINDOWED canvas
 *  height, not a fixed 24h — to land at the right pixel. A short span keeps a
 *  `floorPx` min-height so it stays tappable; a timed task passes its per-effort
 *  floor (GRID_TASK_EFFORT_PX) so a higher-effort task reads as a taller block.
 *  Events use the default MIN_SLOT_PX. */
function slotStyle(p: PositionedSpan, canvasPx: number, floorPx = MIN_SLOT_PX) {
  // The floor itself is capped at the canvas: a degenerate day window shorter
  // than the largest effort floor (e.g. a 1h window vs the 72px large floor)
  // must not push the top clamp negative and draw the chip above the canvas.
  const height = Math.min(
    Math.max(p.heightFraction * canvasPx, floorPx),
    canvasPx,
  );
  // Clamp the TOP (not the height) so a chip near the window's late edge keeps
  // its full height and stays on-canvas — it shifts up a few px rather than
  // being squeezed below the tap target. Clamp by the chip's own (floored)
  // height, matching the desktop reference, so even a large-effort task fits.
  const top = Math.max(0, Math.min(p.topFraction * canvasPx, canvasPx - height));
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
  const [priorityScale, setPriorityScale] = useState<PriorityScale>('three');
  // The synced visible day-window of the hour-grid (`calendar.dayStartMin` /
  // `dayEndMin`, minutes from midnight; default 0/1440 = the full day). The
  // single-day grid spans only [dayStartMin, dayEndMin]: the canvas height +
  // ruler shrink to the window and timed items are positioned window-relative
  // (see computeSlots / layoutDayColumn). Read alongside the effort pref so one
  // focus-refresh covers both; default 0/1440 reproduces today's full-day grid
  // exactly. Window applies to the GRID only — the compact list ignores it.
  const [dayStartMin, setDayStartMin] = useState(0);
  const [dayEndMin, setDayEndMin] = useState(MINUTES_PER_DAY);
  useEffect(() => {
    const read = () =>
      void readTaskBehaviour().then((b) => {
        setEffortSizing(b.visualEffortSizing);
        setPriorityScale(priorityScaleFor(b.twoLevelPriority));
        setDayStartMin(b.dayStartMin);
        setDayEndMin(b.dayEndMin);
      });
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
  // single-day grid caller swaps `days` on prev/next/jump) OR the visible-hours
  // pref changes (a narrower window moves where "now" / the first event sits in
  // the canvas). Clearing the guard and the stale measured offsets lets the next
  // layout pass re-scroll; without this, navigating to a new afternoon-only day
  // would stay parked at the previous day's offset. Grid-only state, but cheap
  // and harmless for the other callers.
  const dayWindowKey = `${dayKeys.join('|')}@${dayStartMin}-${dayEndMin}`;
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

  // ── Windowed hour-grid geometry (dayLayout='grid' only) ──────────────────────
  // Derived from the synced [dayStartMin, dayEndMin] window. Mirrors the desktop
  // DayView: windowMin = the visible span (≥ 1), dayHours = its hours, and the
  // canvas height = dayHours × HOUR_PX so the column shrinks to the window. The
  // shared layoutDayColumn returns WINDOW-RELATIVE fractions for in-window items,
  // so multiplying by `canvasPx` (not a fixed 24h) lands each chip correctly.
  // Default 0/1440 → windowMin 1440, dayHours 24, canvasPx HOUR_PX*24 — the exact
  // pre-window full-day canvas.
  const dayWindow = useMemo(
    () => ({ startMin: dayStartMin, endMin: dayEndMin }),
    [dayStartMin, dayEndMin],
  );
  const windowMin = Math.max(1, dayEndMin - dayStartMin);
  const dayHours = windowMin / 60;
  const canvasPx = dayHours * HOUR_PX;
  // Whole-hour ruler ticks inside the window, INCLUDING the window-end hour so
  // the chosen end is labelled (e.g. 7…23 for a 7–23 window) — but never 24:00,
  // the degenerate full-day end. Positioned by (h*60 - dayStartMin)/windowMin —
  // mirrors desktop's rulerHours.
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

  // A request-epoch guard: the latest load wins. Changing the day window (e.g.
  // the week-start pref resolving async, or a prev/next step) recomputes `range`
  // and re-fires load while an earlier fetch may still be in flight; without
  // this, a slow earlier resolution could overwrite the newer window's data and
  // leave events mismatched against the day headers (derived from `days`).
  // Long-press action menu — the sighted twin of the rows' SR custom actions
  // (one shared action list feeds both). Set by a row's / day header's
  // long-press; one menu instance renders for the whole list.
  const [menu, setMenu] = useState<{
    title: string;
    actions: MenuAction[];
    onAction: (name: string) => void;
  } | null>(null);

  const reqToken = useRef(0);
  // Whether a load has ever completed with data on screen. The FIRST load blanks
  // to the "loading" text (nothing to show yet); every later reload — focus
  // return after an editor, a delete/edit, a background-cache refresh — keeps the
  // current content visible and refreshes in place, so the view stays open
  // desktop-style instead of flashing the loading screen.
  const hasLoadedRef = useRef(false);

  // ── Grid auto-scroll (dayLayout='grid' only; sighted/low-vision nicety) ──────
  // The grid renders a windowed-height canvas (dayHours × HOUR_PX) inside this
  // ScrollView with no initial offset, so an afternoon-only day (full window)
  // would open showing an empty early-morning band. After layout we scroll ONCE
  // per day so the first in-window timed slot (or, on today, the current hour)
  // sits near the top. This is purely
  // visual: it touches no accessibilityLabel/role/action/tap handler, and only
  // the GRID path calls it — list/linear modes stack from the top already.
  const scrollRef = useRef<ScrollView>(null);
  // Native pull-to-refresh — kicks a manual sync + external-cache warm; the list
  // reloads off the resulting cache-update. iOS + Android both back RefreshControl.
  const { refreshing, onRefresh } = usePullRefresh();
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
      // Today → current-hour offset, window-relative + clamped into the canvas
      // (the now-line could be outside a narrow window); otherwise the earliest
      // in-window timed slot. If the day has no in-window timed items
      // (earliestSlotTopPx == null) and it isn't today, leave it at the top —
      // all-day / outside-band / untimed items live there.
      const nowMin = new Date().getHours() * 60 + new Date().getMinutes();
      const nowFrac = Math.min(1, Math.max(0, (nowMin - dayStartMin) / windowMin));
      const withinCanvas = today ? nowFrac * canvasPx : earliestSlotTopPx;
      if (withinCanvas == null) {
        // Mark the day handled so a no-timed-items day doesn't re-check forever.
        scrolledDayKeyRef.current = b.key;
        return;
      }
      const target = Math.max(0, canvasY + withinCanvas - GRID_SCROLL_PAD_PX);
      scrolledDayKeyRef.current = b.key;
      scrollRef.current?.scrollTo({ y: target, animated: false });
    },
    [dayLayout, dayStartMin, windowMin, canvasPx],
  );

  const load = useCallback(async () => {
    const token = (reqToken.current += 1);
    // Blank ONLY the first load; later reloads keep the current content on
    // screen (see hasLoadedRef). The haptic coordinator gives a tactile cue
    // when a load is slow enough to notice, and stays silent on fast warm ones.
    if (!hasLoadedRef.current) setLoading(true);
    setError(null);
    hapticLoadBegin();
    try {
      // listCalendars also primes the Host's route map (getEvents routes by
      // calendar id), so it must resolve before the per-calendar fetch.
      // Palette + lists are best-effort — but a transient failure REUSES the
      // last successful result instead of degrading to empty: replacing the
      // task lists with [] wiped every task (and with it part of the day's
      // announced entry count) until the next reload.
      const [cals, labels, lists] = await Promise.all([
        listCalendars(),
        listColorLabels().catch((err) => {
          console.warn('listColorLabels failed; reusing last known', err);
          return lastKnownColorLabels;
        }),
        listTaskLists().catch((err) => {
          console.warn('listTaskLists failed; reusing last known', err);
          return lastKnownTaskLists;
        }),
      ]);
      // Fence like the retention writes below: a superseded load's slower
      // catalog response must not overwrite the newer load's.
      if (reqToken.current === token) {
        lastKnownColorLabels = labels;
        lastKnownTaskLists = lists;
      }
      const startIso = range.start.toISOString();
      const endIso = range.end.toISOString();
      // Per-container retention (mirror of the desktop useEvents fix): a
      // container whose read FAILS keeps its last successful batch, so one
      // hiccuping backend can't shrink the aggregated day. A successful read
      // replaces the container's slice verbatim — including with an empty
      // batch when the provider really has nothing — so genuine deletions
      // still propagate. Retention writes are fenced on the run being
      // current: a superseded run's slow response landing after a newer
      // run's write would put pre-mutation data back into the cache (a
      // later failure fallback would then resurrect e.g. a deleted event).
      const isCurrent = () => reqToken.current === token;
      const [perCalendar, perList, perListSections] = await Promise.all([
        Promise.all(
          cals.map((c) => {
            const ckey = `${c.id}|${startIso}|${endIso}`;
            return getEvents({ calendar_id: c.id, start: startIso, end: endIso }).then(
              (batch) => {
                if (isCurrent()) retainEvents(ckey, batch);
                return batch;
              },
              (err) => {
                console.warn('getEvents failed for calendar', c.id, err);
                return perCalendarEventsCache.get(ckey) ?? [];
              },
            );
          }),
        ),
        Promise.all(
          lists.map((l) =>
            getTasks(l.id).then(
              (batch) => {
                if (isCurrent()) perListTasksCache.set(l.id, batch);
                return batch;
              },
              (err) => {
                console.warn('getTasks failed for list', l.id, err);
                return perListTasksCache.get(l.id) ?? ([] as Task[]);
              },
            ),
          ),
        ),
        Promise.all(
          lists.map((l) =>
            getSections(l.id).then(
              (batch) => {
                // This screen reads the API directly rather than through the
                // task store, so the store's sort does not reach it — see
                // `sortSections`. Sorted on the way INTO the cache so a cache
                // hit and a fresh fetch read the same.
                const sorted = sortSections(batch);
                if (isCurrent()) perListSectionsCache.set(l.id, sorted);
                return sorted;
              },
              (err) => {
                console.warn('getSections failed for list', l.id, err);
                return perListSectionsCache.get(l.id) ?? ([] as Section[]);
              },
            ),
          ),
        ),
      ]);
      // A newer load superseded this one — drop these stale results.
      if (reqToken.current !== token) return;
      setCalendars(cals);
      setColorLabels(labels);
      setTaskLists(lists);
      // Expand recurring series across the whole window so an event recurring
      // mid-window isn't invisible after its first occurrence (rrule + EXDATE).
      //
      // EVERYTHING, including the meetings a videoconference account
      // contributes for appointments that already have a calendar entry.
      // Hiding those duplicates happens further down, once the groups are
      // known — because the honest way to hide one is to group the two, and
      // pairing them needs both rows (DESIGN-event-groups.md, Stufe 4).
      setEvents(
        expandAll(perCalendar.flat(), { start: range.start, end: range.end }),
      );
      setTasks(perList.flat());
      setSections(perListSections.flat());
      hasLoadedRef.current = true;
    } catch (err) {
      if (reqToken.current !== token) return;
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      if (reqToken.current === token) setLoading(false);
      // Balances the hapticLoadBegin above — always, even for a superseded load.
      hapticLoadEnd();
    }
  }, [announce, range, t]);

  // Coalesce reload requests: the two cache-reload subscriptions (calendar +
  // tasks) usually fire TOGETHER (a warm pass refreshes both) and focus can race
  // them, so without this each triggers its own full, SERIALIZED FFI pass over
  // every calendar + list. It ALSO coalesces rapid PAGING: a VoiceOver
  // three-finger swipe (or the ‹ › buttons held down) steps the period fast, and
  // reloading the whole day/week/month on every step churned VoiceOver's
  // accessibility tree mid-swipe — delaying the period announcement and blocking
  // the next swipe. Debouncing so only the period the user SETTLES on reloads
  // keeps swiping churn-free; the period announcement is immediate (CalendarPager)
  // and independent of the data, so nothing waits on this. The window is a bit
  // longer than a fast swipe interval for that reason; `reqToken` still drops any
  // stale result. The FIRST mount load stays immediate for a fast first paint.
  const loadTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scheduleLoad = useCallback(() => {
    if (loadTimer.current != null) clearTimeout(loadTimer.current);
    loadTimer.current = setTimeout(() => {
      loadTimer.current = null;
      void load();
    }, RELOAD_DEBOUNCE_MS);
  }, [load]);
  useEffect(
    () => () => {
      if (loadTimer.current != null) clearTimeout(loadTimer.current);
    },
    [],
  );

  // Reload when the window changes or the screen regains focus (after an editor).
  // First mount loads immediately (fast first paint); every LATER change — most
  // importantly a page step, which recomputes `range` — goes through the debounce
  // so rapid paging collapses into one reload on the period the user settles on.
  const didFirstLoadRef = useRef(false);
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => scheduleLoad());
    if (!didFirstLoadRef.current) {
      didFirstLoadRef.current = true;
      void load();
    } else {
      scheduleLoad();
    }
    return unsubscribe;
  }, [navigation, load, scheduleLoad]);

  // Live-update while focused: this view loads BOTH events and tasks per day, so
  // re-read on either an external calendar- or task-cache refresh — coalesced via
  // scheduleLoad so a paired refresh doesn't fire two full passes.
  useCacheReload('calendar', scheduleLoad);
  useCacheReload('tasks', scheduleLoad);

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
  // Which of these Aperio has been told mean the same appointment. One query
  // per window, not one per row; whole groups come back, so a copy in a
  // switched-off calendar still counts toward what a folded row says.
  const [eventGroups, setEventGroups] = useState<EventGroup[]>([]);
  // Pairs this mount has already tried to group automatically — see below.
  const linkAttempted = useRef(new Set<string>());
  useEffect(() => {
    let cancelled = false;
    const refs = visibleEvents.map((ev) => ({
      calendar_id: ev.calendar_id,
      event_id: seriesIdOf(ev),
    }));
    if (refs.length === 0) {
      setEventGroups([]);
      return;
    }
    eventGroupsForEvents(refs)
      .then(async (found) => {
        if (cancelled) return;
        // Keep the signatures describing the events as they ARE. Written once
        // at joining, they went stale the moment the appointment moved — and
        // the repair below searches by exactly them.
        for (const stale of findStaleSignatures(found, visibleEvents, seriesIdOf)) {
          await refreshEventGroupSignature(stale).catch(() => undefined);
        }
        if (cancelled) return;
        // Repair members whose provider id changed, while the range that proves
        // it is in hand: a member whose stored start falls inside it and whose
        // id resolves to nothing here has been re-minted, and the signature
        // says which event it is now. Silent — the same events mean the same
        // appointment before and after (DESIGN-event-groups.md).
        const healable = findHealableMembers(found, visibleEvents, range, seriesIdOf);
        for (const member of healable) {
          await healEventGroupMember(member).catch(() => undefined);
        }
        if (cancelled) return;
        // Read back rather than patch locally: the stored group is the answer.
        let fresh = healable.length
          ? await eventGroupsForEvents(refs).catch(() => found)
          : found;
        // Group a videoconference meeting with the appointment it belongs to.
        //
        // Unlike the two repairs above this WRITES a group, and the group
        // syncs — it is a statement about what an appointment is, not
        // bookkeeping. It rests on the join URL, an identity the provider
        // issued, which is why it may happen without being asked while
        // name-and-time resemblance may not (DESIGN-event-groups.md, Stufe 4).
        const declines = await groupSuggestionDeclines().catch(() => null);
        if (cancelled) return;
        if (declines != null) {
          const pairs = findMeetingLinkPairs(
            visibleEvents,
            fresh,
            declines,
            seriesIdOf,
          ).filter((pair) => {
            // Once per pair per mount: a pair that cannot be grouped (the two
            // turn out to be in different groups, say) must be tried once and
            // then left alone, or every reload would ask again.
            const key = `${eventGroupMemberKey(pair.meeting.calendar_id, seriesIdOf(pair.meeting))}\n${eventGroupMemberKey(pair.event.calendar_id, seriesIdOf(pair.event))}`;
            if (linkAttempted.current.has(key)) return false;
            linkAttempted.current.add(key);
            return true;
          });
          let grouped = false;
          for (const pair of pairs) {
            try {
              await groupEvents([
                memberFromEvent({ ...pair.meeting, id: seriesIdOf(pair.meeting) }),
                memberFromEvent({ ...pair.event, id: seriesIdOf(pair.event) }),
              ]);
              grouped = true;
            } catch {
              // Refused or not written. The day looks exactly as it did — the
              // filter still hides the duplicate — and this pair is not asked
              // about again.
            }
          }
          if (cancelled) return;
          if (grouped) fresh = await eventGroupsForEvents(refs).catch(() => fresh);
        }
        if (!cancelled) setEventGroups(fresh);
      })
      .catch(() => {
        // No folding this round — which is what the app did before groups
        // existed. Never an empty day.
        if (!cancelled) setEventGroups([]);
      });
    return () => {
      cancelled = true;
    };
  }, [visibleEvents, range]);
  // What the days actually draw: the same rows, minus a videoconference
  // meeting whose appointment is in view and NOT grouped with it. A grouped
  // one stays — folding hides it then, and folding hides it while counting it,
  // and stops hiding it the moment the two disagree about when the appointment
  // is. That last case is the one this filter can never see on its own: the
  // join URL still matches after a move, so it would hide the meeting exactly
  // when the mismatch matters (DESIGN-event-groups.md, Stufe 4).
  const renderEvents = useMemo(() => {
    const byMember = indexEventGroups(eventGroups);
    return withoutDuplicateMeetings(visibleEvents, (ev) =>
      byMember.has(eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev))),
    );
  }, [visibleEvents, eventGroups]);
  // Expand recurring SCHEDULED tasks into one occurrence per planned day across
  // the visible window — so a task recurring every day/week shows on EVERY due
  // day here (like a recurring event), not only its single current
  // scheduled_date. The occurrence on the task's own date is the real,
  // interactive task; the others are read-only projections (isRecurringProjection)
  // that route to the series on tap and offer no complete/delete (the current
  // instance advances the series on completion — mirrors the backend spawner).
  // Non-recurring / from-completion / backlog tasks pass through untouched.
  const expandedTasks = useMemo(() => {
    if (dayKeys.length === 0) return tasks;
    let fromKey = dayKeys[0];
    let toKey = dayKeys[0];
    for (const k of dayKeys) {
      if (k < fromKey) fromKey = k;
      if (k > toKey) toKey = k;
    }
    return expandScheduledRecurringTasks(tasks, fromKey, toKey);
  }, [tasks, dayKeys]);
  // The day buckets AND what each folded row stands for, built together: the
  // map describes exactly these rows, so computing it anywhere else would let
  // a stale one outlive them.
  const { buckets, groupRows } = useMemo(() => {
    const rows = new Map<string, CollapsedRow<CalendarEvent>>();
    const built: DayBucket[] = days.map((date, i) => {
      const key = dayKeys[i];
      const allDay = renderEvents.filter(
        (ev) => ev.all_day && daysCoveredKeys(ev).includes(key),
      );
      const timedEvents = renderEvents.filter(
        // daysCoveredKeys spreads a timed event across midnight too, so a
        // 23:00→01:00 meeting buckets onto both days (mergeDayItems +
        // eventSpanForDay clamp each day's portion).
        (ev) => !ev.all_day && daysCoveredKeys(ev).includes(key),
      );
      const dayTasks = filterTasksOnDay(
        expandedTasks,
        key,
        showCompletedForList,
        meFor,
        priorityScale,
      );
      // One row per appointment instead of one per copy, decided PER DAY —
      // the contract `collapseEventGroups` documents, because a recurring
      // appointment renders a row per day and across a week its own days
      // would look exactly like copies that disagree.
      //
      // ONE call over the whole day, then split back. Folding all-day and
      // timed separately hid a real divergence: a group whose copies are
      // all-day in one calendar and timed in another disagrees about when the
      // appointment is, and split into two calls neither half could see it —
      // both folded quietly, and the desktop reported what the phone did not.
      const foldedDay = collapseEventGroups(
        [...allDay, ...timedEvents],
        eventGroups,
        seriesIdOf,
      );
      for (const row of foldedDay) {
        if (row.group) rows.set(`${eventInstanceKey(row.event)}@${key}`, row);
      }
      const foldedAllDay = foldedDay.filter((row) => row.event.all_day);
      const foldedTimed = foldedDay.filter((row) => !row.event.all_day);
      const { timed, untimed } = mergeDayItems(
        foldedTimed.map((row) => row.event),
        dayTasks,
        key,
        (ev) => new Date(ev.start).getTime(),
      );
      return {
        key,
        date,
        allDay: foldedAllDay.map((row) => row.event),
        timed,
        untimed,
        // The FOLDED all-day rows: the header is what a screen-reader user
        // lands on when jumping by heading, and it counted copies the list
        // below no longer shows.
        count: foldedAllDay.length + timed.length + untimed.length,
      };
    });
    return { buckets: built, groupRows: rows };
  }, [
    days,
    dayKeys,
    renderEvents,
    expandedTasks,
    eventGroups,
    showCompletedForList,
    meFor,
    priorityScale,
  ]);

  const totalItems = useMemo(
    () => buckets.reduce((sum, b) => sum + b.count, 0),
    [buckets],
  );

  const editEvent = useCallback(
    (ev: CalendarEvent) =>
      // A recurring occurrence pops the "this occurrence vs whole series" prompt
      // first, then opens the editor locked to the choice (shared helper).
      editEventWithScope(ev, t, (params) =>
        navigation.navigate('EventEditor', params),
      ),
    [navigation, t],
  );

  // Move / copy to another calendar — pass the full (possibly expanded) row so
  // the modal can offer the occurrence-vs-series scope.
  const moveCopyEvent = useCallback(
    (ev: CalendarEvent) => navigation.navigate('MoveCopy', { kind: 'event', event: ev }),
    [navigation],
  );

  const groupEvent = useCallback(
    (ev: CalendarEvent) => navigation.navigate('EventGroup', { event: ev }),
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
        {
          supportsScheduling:
            calendars.find((c) => c.id === ev.calendar_id)
              ?.supports_scheduling ?? false,
        },
      ),
    [announce, calendars, load, t],
  );

  const openTask = useCallback(
    (task: Task) =>
      // recurringSeriesTaskId strips a projection's occurrence suffix (and is a
      // no-op for a real task id), so tapping a read-only recurring projection
      // opens the underlying series, never a non-existent occurrence id.
      navigation.navigate('TaskEditor', {
        taskId: recurringSeriesTaskId(task.id),
        listId: task.list_id,
      }),
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

  // "Plan" — deciding WHEN a task happens, without opening the editor. The
  // task list has offered it for a while; the calendar's task rows are where
  // rescheduling is most often decided, and they did not.
  const planTask = useCallback(
    (task: Task) =>
      navigation.navigate('PlanTask', { taskId: task.id, listId: task.list_id }),
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
      const plainConfirm = () =>
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
      // A recurring DEVICE reminder gets the "only this / this and all following"
      // scope choice (iOS-Reminders parity); everything else takes the plain
      // confirm above.
      confirmDeleteTask(
        task,
        taskLists,
        t,
        (message) => {
          announce(message);
          void load();
        },
        (message) => announce(t('mobile.error', { message })),
        plainConfirm,
      );
    },
    [announce, load, t, taskLists],
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
      // What this row stands for, if it stands for more than itself. The count
      // comes from the group, so a copy in a switched-off calendar is counted
      // too — that is what makes it match what the user knows they keep.
      const groupRow = groupRows.get(`${eventInstanceKey(ev)}@${localDateKey(day)}`);
      if (groupRow?.group) {
        label += groupRow.diverged
          ? t('views.eventGroupDivergedSuffix', { count: groupRow.otherMembers })
          : t('views.eventGroupSuffix', {
              count: groupRow.otherMembers,
              calendars: groupRow.calendarIds
                .map((id) => calendarsById.get(id)?.name ?? id)
                .join(', '),
            });
      }
      if (ev.cancelled) {
        label += t('views.eventCancelledSuffix');
      }
      const colour = resolveEventColor(ev, calendarsById, labelsById);
      if (colour.labelName) {
        label += t('mobile.colorLabelSuffix', { name: colour.labelName });
      }
      return label;
    },
    [calendarsById, eventTimeLabel, groupRows, labelsById, t],
  );

  const taskLabel = useCallback(
    (task: Task, key: string, colourName: string | null): string => {
      const time = taskTimeOnDay(task, key);
      const common = {
        title: task.title,
        state: t(statusI18nKey(task.status)),
        priority: prioritySuffix(tr, task.priority, priorityScale),
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
      if (task.recurrence) {
        // Announced for ANY recurring task, not only for a read-only future
        // occurrence. Gating it on the projection meant the row you can
        // actually act on — today's instance — was the one row that never said
        // it repeats: a daily task read "open, planned" today and "recurring"
        // on every other day. That a projection cannot be ticked off is carried
        // by the row itself, which offers no checkbox and only "open the task".
        label += t('views.tasks.recurringOccurrence');
      }
      // Effort suffix appended unconditionally (regardless of the visual-sizing
      // toggle) so a screen reader always hears it; '' for medium.
      return label + effortSuffix(tr, task.effort) + subtaskParentSuffix(tr, task, tasks);
    },
    [fmtDateOnly, fmtTime, t, tasks, tr, priorityScale],
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
    const rowKey = `e-${eventInstanceKey(ev)}@${localDateKey(day)}`;
    const hex = resolveEventColor(ev, calendarsById, labelsById).hex;
    const grid = slot != null;
    // Sighted colour: TINT the whole tile in the event's resolved colour (the
    // per-tester ask; replaces the old colour dot) — a low-alpha fill keeps the
    // text contrast on both themes, the stronger border keeps the hue readable
    // on tiny grid chips. SR users get the label NAME in eventLabel unchanged.
    const tint =
      hex != null ? { backgroundColor: `${hex}2E`, borderColor: `${hex}66` } : null;
    // Cancelled events (shown when "show cancelled events" is on): dim the tile +
    // strike the title, matching desktop. SR users get ", abgesagt" in the label.
    const cancelledTile = ev.cancelled ? styles.cancelledTile : null;
    const titleStyle = ev.cancelled
      ? [styles.itemTitle, styles.cancelledTitle]
      : styles.itemTitle;
    const badge = span
      ? ` ${t('views.multiDayCompact', { day: span.dayIndex, total: span.totalDays })}`
      : '';
    // What this row stands for, for the EYE. The label already says it in
    // words, so the mark below is hidden from the reader — without it, folding
    // was audible and invisible, and a group that had drifted apart looked
    // like two unrelated rows.
    const groupRow = groupRows.get(`${eventInstanceKey(ev)}@${localDateKey(day)}`);
    const group = groupRow ? groupBadge(groupRow) : null;
    // Read-only calendars still get the join affordance when the event carries
    // a meeting: these rows have no editor to open, so without it a meeting on
    // a subscribed or shared calendar would be un-joinable from the app at all.
    // Everything else about the row stays read-only.
    const readOnlyJoin = readOnlyIds.has(ev.calendar_id) ? joinAction(ev, t) : null;
    if (readOnlyIds.has(ev.calendar_id)) {
      // Grouping belongs here too, and read-only is exactly why: a colleague's
      // calendar is the one Aperio can never write to, and its copy of the
      // meeting is the copy that shows up twice. The group is Aperio's own
      // statement ABOUT the event — it changes nothing on the provider, so
      // there is nothing here that read-only should forbid.
      const readOnlyActions: MenuAction[] = [
        ...(readOnlyJoin ? [readOnlyJoin.action] : []),
        {
          name: 'group',
          // // "Belongs together with…" reads like nothing has been grouped yet. On an
          // event that IS grouped, that hid the fact that this is also where the
          // group is read, added to and taken apart — so the entry says which of
          // the two it does. The groups are already in hand for the folding, so
          // this costs no lookup.
          label: t(groupRow?.group ? 'chipMenu.manageGroup' : 'chipMenu.groupWith'),
        },
      ];
      const runReadOnlyAction = (name: string) => {
        if (name === 'group') groupEvent(ev);
        else if (readOnlyJoin) void openConference(readOnlyJoin.conference, t);
      };
      return (
        <View
          key={rowKey}
          accessible
          accessibilityRole="text"
          accessibilityLabel={eventLabel(ev, day, span)}
          accessibilityActions={readOnlyActions}
          onAccessibilityAction={(e) => runReadOnlyAction(e.nativeEvent.actionName)}
          style={
            grid
              ? [styles.gridChip, slotStyle(slot, canvasPx), tint]
              : [styles.row, extraStyle, tint]
          }
        >
          <Pressable
            accessible={false}
            onLongPress={() =>
              setMenu({
                title: ev.title,
                actions: readOnlyActions,
                onAction: runReadOnlyAction,
              })
            }
            style={styles.rowText}
          >
            <Text
              style={titleStyle}
              importantForAccessibility="no"
              numberOfLines={grid ? 1 : undefined}
            >
              {ev.title}
              {badge}
              {group != null && (
                <Text
                  style={
                    groupRow?.diverged ? styles.groupBadgeDiverged : styles.groupBadge
                  }
                  importantForAccessibility="no"
                >
                  {` ${group}`}
                </Text>
              )}
            </Text>
            {!grid && (
              <Text style={styles.itemMeta} importantForAccessibility="no">
                {eventTimeLabel(ev, day)}
              </Text>
            )}
          </Pressable>
        </View>
      );
    }
    // ONE action list feeds the SR custom actions AND the sighted long-press
    // menu; the per-row delete button is gone (delete lives in the editor, the
    // menu and the SR action — the visible button was row clutter).
    // A meeting link, if the event carries one, leads the list: a row with a
    // meeting on it is far more often opened to JOIN than to edit, and this
    // way joining costs one rotor step from the calendar instead of opening
    // the editor. Provider- and language-independent, so an invitation from
    // Outlook or eM Client offers it just the same.
    const join = joinAction(ev, t);
    const actions: MenuAction[] = [
      ...(join ? [join.action] : []),
      { name: 'activate', label: t('mobile.editTaskLabel') },
      { name: 'moveCopy', label: t('mobile.moveCopy') },
      // The entry point to event groups. It sits with the non-destructive
      // verbs because it is a statement ABOUT the event, not something done to
      // it: no provider hears of it, and taking it back leaves nothing
      // changed. Named by state — see the read-only list above.
      {
        name: 'group',
        label: t(groupRow?.group ? 'chipMenu.manageGroup' : 'chipMenu.groupWith'),
      },
      { name: 'delete', label: t('dialogs.event.delete'), destructive: true },
    ];
    const runAction = (name: string) => {
      if (name === 'join' && join) void openConference(join.conference, t);
      else if (name === 'delete') removeEvent(ev);
      else if (name === 'moveCopy') moveCopyEvent(ev);
      else if (name === 'group') groupEvent(ev);
      else editEvent(ev);
    };
    return (
      <View
        key={rowKey}
        accessible
        accessibilityRole="button"
        accessibilityLabel={eventLabel(ev, day, span)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={actions}
        onAccessibilityAction={(e) => runAction(e.nativeEvent.actionName)}
        style={
          grid
            ? [styles.gridChip, slotStyle(slot, canvasPx), tint, cancelledTile]
            : [styles.row, extraStyle, tint, cancelledTile]
        }
      >
        <Pressable
          accessible={false}
          onPress={() => editEvent(ev)}
          onLongPress={() => setMenu({ title: ev.title, actions, onAction: runAction })}
          style={styles.rowText}
        >
          <Text
            style={titleStyle}
            importantForAccessibility="no"
            numberOfLines={grid ? 1 : undefined}
          >
            {ev.title}
            {badge}
            {group != null && (
              <Text
                style={
                  groupRow?.diverged ? styles.groupBadgeDiverged : styles.groupBadge
                }
                importantForAccessibility="no"
              >
                {` ${group}`}
              </Text>
            )}
          </Text>
          {!grid && (
            <Text style={styles.itemMeta} importantForAccessibility="no">
              {eventTimeLabel(ev, day)}
            </Text>
          )}
        </Pressable>
      </View>
    );
  };

  const renderTaskRow = (task: Task, key: string, slot?: PositionedSpan) => {
    const done = task.status === 'completed';
    const grid = slot != null;
    const resolved = resolveTaskColor(task, listsById, labelsById, sectionColorById);
    const hex = resolved.hex;
    // Visual tile-size by effort (sighted users), only when the synced pref is
    // on. The scale sits one step above the original mapping (tester
    // feedback), so SMALL is the neutral base row height (the modifier
    // returns '' for it). Purely cosmetic — the effort is always in the
    // row's accessibilityLabel via effortSuffix.
    // In the LINEAR list (and the compact list-view) this is a row style
    // (rowEffortMedium/Large). In the hour-GRID a chip is absolutely
    // positioned by time, so we can't restyle its row height freely; instead
    // we RAISE its slot FLOOR per effort via slotStyle's `floorPx` below —
    // the slot's TOP (its time position) is preserved, so higher-effort
    // tasks read as taller blocks without drifting off their time.
    const effortStyle =
      !grid && effortSizing
        ? effortSizeModifier(task.effort) === 'medium'
          ? styles.rowEffortMedium
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
    // A read-only recurring PROJECTION (a future planned occurrence of a
    // recurring task) is a preview only: completion / reschedule / delete act on
    // the CURRENT instance (the real base row on its own scheduled day), which
    // advances the series when checked off. So a projection offers just "open the
    // task" — routed to the underlying series (recurringSeriesTaskId) via
    // openTask — and shows a non-interactive recurrence marker instead of a
    // checkbox.
    const projection = isRecurringProjection(task);
    // ONE action list feeds the SR custom actions AND the sighted long-press
    // menu (same model as the event rows).
    const actions: MenuAction[] = projection
      ? [{ name: 'edit', label: t('mobile.editTaskLabel') }]
      : [
          { name: 'toggle', label: done ? t('mobile.reopen') : t('mobile.complete') },
          { name: 'edit', label: t('mobile.rename') },
          // Not for a recurring PROJECTION: it is a read-only shadow of its
          // series, and planning one occurrence of it is not a thing the
          // series can hold.
          { name: 'plan', label: t('mobile.plan') },
          { name: 'moveCopy', label: t('mobile.moveCopy') },
          { name: 'delete', label: t('mobile.delete'), destructive: true },
        ];
    const runAction = (name: string) => {
      if (projection) {
        openTask(task);
        return;
      }
      if (name === 'toggle') void toggleTask(task);
      else if (name === 'delete') removeTask(task);
      else if (name === 'moveCopy') moveCopyTask(task);
      else if (name === 'plan') planTask(task);
      else openTask(task);
    };
    return (
      <View
        key={`t-${task.id}@${key}`}
        accessible
        accessibilityRole="button"
        accessibilityLabel={taskLabel(task, key, resolved.labelName)}
        accessibilityHint={t('mobile.taskHint')}
        accessibilityActions={actions}
        onAccessibilityAction={(e) => runAction(e.nativeEvent.actionName)}
        style={
          grid
            ? [
                styles.gridChip,
                slotStyle(
                  slot,
                  canvasPx,
                  effortSizing ? GRID_TASK_EFFORT_PX[task.effort] : MIN_SLOT_PX,
                ),
              ]
            : [styles.row, effortStyle]
        }
      >
        {/* Sighted tap target to complete/reopen the task; the row otherwise
            opens the task on tap, so the marker needs its own Pressable. SR
            users use the row's "toggle" custom action, so this stays out of the
            accessibility tree. A read-only recurring projection shows a
            non-interactive recurrence glyph instead (it has no completion of its
            own — the current instance owns that). */}
        {projection ? (
          <View
            accessible={false}
            importantForAccessibility="no"
            style={styles.taskCheckButton}
          >
            <Text style={styles.taskCheck} importantForAccessibility="no">
              ↻
            </Text>
          </View>
        ) : (
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
        )}
        {hex != null && (
          <View
            accessible={false}
            importantForAccessibility="no"
            style={[styles.colorDot, { backgroundColor: hex }]}
          />
        )}
        <Pressable
          accessible={false}
          onPress={() => openTask(task)}
          onLongPress={() => setMenu({ title: task.title, actions, onAction: runAction })}
          style={styles.rowText}
        >
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
    const positions = layoutDayColumn(spans, dayWindow);
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

  // One timed item rendered as a plain (un-slotted) row for an "outside the
  // visible hours" band — reuses renderEventRow/renderTaskRow WITHOUT a slot, so
  // the row keeps its full accessibilityLabel (with its REAL time — a cross-
  // midnight event shows its clamped this-day time via eventTimeLabel), all its
  // custom actions and its tap-to-open handler. Unlike the desktop (which clips a
  // listbox option and shows an aria-hidden bar), an RN band row is itself the
  // accessible element, so the screen reader reaches every outside item here.
  const renderOutsideRow = (
    item: DayGridItem<CalendarEvent, Task>,
    day: Date,
    key: string,
  ): ReactNode =>
    item.kind === 'event'
      ? renderEventRow(item.event, day, multiDayInfo(item.event, day))
      : renderTaskRow(item.task, key);

  // Single-day hour-grid: all-day events, then (when the window hides them) a
  // compact BEFORE band of items earlier than the window start, then the windowed
  // canvas (a leading hour-ruler beside the positioned day column), then an AFTER
  // band of items past the window end, then a compact band of UNTIMED tasks BELOW
  // the canvas (per Toni: the task band stays under the grid). Reading/source
  // order: all-day → before → timed(canvas) → after → untimed → create buttons
  // (events first, then tasks) — and `b.timed` is already chronological, so the
  // before/in/after split preserves time order across the three groups. Each band
  // row uses the SAME renderEventRow/renderTaskRow (no slot), so its
  // accessibilityRole/Label/actions/effort sizing + tap handlers are unchanged.
  // The grid is visual only; within the canvas timed order is preserved.
  const renderDayGrid = (b: DayBucket): ReactNode => {
    const slots = computeSlots(b);
    // Partition the day's timed items by where the shared layout placed them
    // relative to the visible window. In-window items keep their canvas slot;
    // before/after items fall into the outside bands above/below the grid. Items
    // with no slot (defensive) are treated as in-window so nothing is dropped.
    const before: { item: DayGridItem<CalendarEvent, Task> }[] = [];
    const inWindow: { item: DayGridItem<CalendarEvent, Task>; idx: number }[] = [];
    const after: { item: DayGridItem<CalendarEvent, Task> }[] = [];
    b.timed.forEach((item, idx) => {
      const placement = slots.get(idx)?.placement ?? 'in';
      if (placement === 'before') before.push({ item });
      else if (placement === 'after') after.push({ item });
      else inWindow.push({ item, idx });
    });
    // The earliest IN-WINDOW slot's top in px (the highest chip on the canvas), or
    // null when no timed item lands in the window. Drives the auto-scroll target
    // for a non-today day; topFraction is window-relative, so × canvasPx is the
    // chip's top in the (windowed) canvas. Outside items are excluded (their
    // fractions are 0 and they live in the bands, not on the canvas).
    const slotTops = inWindow
      .map(({ idx }) => slots.get(idx))
      .filter((p): p is PositionedSpan => p != null && p.placement === 'in')
      .map((p) => p.topFraction * canvasPx);
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
        {/* Items earlier than the window start — a compact band above the grid.
            Real accessible rows (the time is in each label), so the screen reader
            still reaches an out-of-window event/task. */}
        {before.length > 0 && (
          <View style={styles.outsideBand}>
            <Text accessibilityRole="header" style={styles.outsideBandHeading}>
              {t('views.day.outsideBefore', { time: fmtTime(dayMinuteDate(b.date, dayStartMin)) })}
            </Text>
            {before.map(({ item }) => renderOutsideRow(item, b.date, b.key))}
          </View>
        )}
        <View
          style={styles.gridRow}
          // y of the hour-grid row within the day section (below the all-day band
          // + before-band). daySectionY + this = the canvas top in the scroll
          // content.
          onLayout={(e) => {
            gridRowYRef.current = e.nativeEvent.layout.y;
            maybeScrollGrid(b, earliestSlotTopPx);
          }}
        >
          {/* Hour ruler — the whole-hour numbers INSIDE the window, read off the
              grid instead of the chips. Decorative: the time stays in each row's
              accessibilityLabel, so it's hidden from the screen reader. Each tick
              is positioned by (h*60 - dayStartMin)/windowMin, matching desktop. */}
          <View
            style={[styles.ruler, { height: canvasPx }]}
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
          >
            {rulerHours.map((h) => (
              <Text
                key={h}
                style={[styles.rulerHour, { top: ((h * 60 - dayStartMin) / windowMin) * canvasPx }]}
              >
                {String(h).padStart(2, '0')}
              </Text>
            ))}
          </View>
          {/* The positioned day column, sized to the visible window. Each timed
              chip is absolutely placed by its window-relative start+duration;
              faint gridlines on the whole hours read the column as a grid. */}
          <View style={[styles.canvas, { height: canvasPx }]}>
            {rulerHours.map((h) => (
              <View
                key={h}
                accessibilityElementsHidden
                importantForAccessibility="no-hide-descendants"
                style={[styles.gridLine, { top: ((h * 60 - dayStartMin) / windowMin) * canvasPx }]}
              />
            ))}
            {inWindow.map(({ item, idx }) =>
              item.kind === 'event'
                ? renderEventRow(item.event, b.date, multiDayInfo(item.event, b.date), slots.get(idx))
                : renderTaskRow(item.task, b.key, slots.get(idx)),
            )}
          </View>
        </View>
        {/* Items past the window end — the mirror band below the grid. */}
        {after.length > 0 && (
          <View style={styles.outsideBand}>
            <Text accessibilityRole="header" style={styles.outsideBandHeading}>
              {t('views.day.outsideAfter', {
                time: dayEndMin >= MINUTES_PER_DAY ? '24:00' : fmtTime(dayMinuteDate(b.date, dayEndMin)),
              })}
            </Text>
            {after.map(({ item }) => renderOutsideRow(item, b.date, b.key))}
          </View>
        )}
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
          refreshControl={
            <RefreshControl refreshing={refreshing} onRefresh={onRefresh} />
          }
        >
          {/* Above the days, and only when there is something to ask: the whole
              proactive surface of event groups is one dismissible row. The
              single-day surfaces are where a duplicate is actually noticed;
              multi-day views would ask about a day the user is not reading. */}
          {dayLayout != null && buckets.length === 1 && (
            <GroupSuggestionNotice
              events={renderEvents}
              groups={eventGroups}
              calendars={calendars}
              onChanged={() => void load()}
            />
          )}
          {buckets.map((b) => {
            // Single-day paths. Gated on the explicit `dayLayout` prop, which
            // ONLY the single-day caller (EventsScreen) passes — multi-day
            // Week/Month/Agenda pass nothing and fall through to the linear list
            // below, unchanged.
            if (dayLayout === 'grid') return renderDayGrid(b);
            if (dayLayout === 'list') return renderDayList(b);
            const rows: ReactNode[] = [];
            if (showDayHeaders) {
              // Day-anchored create moved INTO the header (SR custom actions +
              // sighted long-press) — the per-day "+ new event / task" buttons
              // repeated under EVERY day were toolbar duplicates (tester
              // feedback); the function survives without the clutter.
              const headerActions: MenuAction[] = [
                ...(firstWritableCalendarId != null
                  ? [{ name: 'newEvent', label: t('toolbar.newEvent') }]
                  : []),
                ...(firstWritableTaskListId != null
                  ? [{ name: 'newTask', label: t('toolbar.newTask') }]
                  : []),
              ];
              const runHeaderAction = (name: string) => {
                if (name === 'newEvent') addEventOnDay(b.key);
                else if (name === 'newTask') addTaskOnDay(b.key);
              };
              // Hosted in a Pressable (not a bare Text): every device-proven
              // accessibilityActions site in this app sits on a View/Pressable,
              // and custom actions on a raw Text host have platform quirks —
              // this is the ONLY per-day create path here, so it rides the
              // proven pattern. role="header" keeps the headings rotor.
              rows.push(
                <Pressable
                  key={`h-${b.key}`}
                  accessible
                  accessibilityRole="header"
                  accessibilityLabel={t(dayAnnounceKey, {
                    day: fmtFullDate(b.date),
                    count: b.count,
                  })}
                  accessibilityActions={headerActions}
                  onAccessibilityAction={(e) => runHeaderAction(e.nativeEvent.actionName)}
                  onLongPress={
                    headerActions.length > 0
                      ? () =>
                          setMenu({
                            title: fmtFullDate(b.date),
                            actions: headerActions,
                            onAction: runHeaderAction,
                          })
                      : undefined
                  }
                >
                  <Text style={styles.dayHeader} importantForAccessibility="no">
                    {fmtFullDate(b.date)}
                  </Text>
                </Pressable>,
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
                {/* Per-day create buttons only on a SINGLE-day surface (the day
                    view); on Week/Month they repeated under every day — the
                    header's long-press / SR actions carry the day-anchored
                    create there instead. */}
                {days.length === 1 && renderDayCreateButtons(b)}
              </View>
            );
          })}
        </ScrollView>
      )}

      {/* The long-press action menu (one instance for the whole list). */}
      <ActionsMenu
        visible={menu != null}
        title={menu?.title ?? ''}
        actions={menu?.actions ?? []}
        onAction={menu?.onAction ?? (() => undefined)}
        onClose={() => setMenu(null)}
      />
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    scroll: { flex: 1 },
    list: { gap: chrome(8), padding: chrome(12) },
    daySection: { gap: 8 },
    newEventButton: {
      paddingVertical: chrome(8),
      paddingHorizontal: chrome(12),
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
      gap: chrome(10),
      padding: chrome(12),
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    // Effort-driven chip height (gated on the visualEffortSizing pref). One
    // step above the original mapping (tester feedback): small uses the base
    // `row` size, medium the former large size, large grew beyond it.
    // Chrome-scaled in lockstep with TasksScreen's effort tiles.
    rowEffortMedium: { paddingVertical: chrome(20), minHeight: chrome(88) },
    rowEffortLarge: { paddingVertical: chrome(28), minHeight: chrome(112) },
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
    // Cancelled event: dim the tile + strike the title (matches desktop).
    cancelledTile: { opacity: 0.6 },
    cancelledTitle: { textDecorationLine: 'line-through' as const },
    itemTitleDone: { textDecorationLine: 'line-through', color: c.textSecondary },
    itemMeta: { fontSize: 14, color: c.textSecondary },
    // The mark on a row standing for several copies of one appointment ("3×").
    // Hidden from the reader — the row's label says it in words.
    groupBadge: { fontSize: 13, color: c.textSecondary },
    // Copies that no longer agree about the time. Coloured, because that is a
    // state to act on rather than a fact about the row.
    groupBadgeDiverged: { fontSize: 13, color: c.warning, fontWeight: '600' as const },
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
    // ── Single-day "outside the visible hours" bands (dayLayout='grid') ───────
    // The compact band of events/tasks that fall before the window start (above
    // the grid) or after the window end (below it). Same row renderers as the
    // canvas, so each band row is a full accessible row (the time is in its
    // label) — the screen reader still reaches every out-of-window item.
    outsideBand: { gap: 4 },
    outsideBandHeading: {
      fontSize: 13,
      fontWeight: '700',
      color: c.textLabel,
      marginBottom: 2,
    },
    // ── Single-day hour-grid (dayLayout='grid') ──────────────────────────────
    // A horizontal [ruler | canvas] row; the canvas is a windowed-height
    // positioned column (its height is set inline per-render = dayHours × HOUR_PX,
    // shrinking to the visible [dayStartMin, dayEndMin] window). Ruler numbers +
    // chips line up because both are offset from the SAME top by the same
    // window-relative fraction × that height.
    gridRow: { flexDirection: 'row', alignItems: 'flex-start' },
    ruler: { width: RULER_PX, position: 'relative' },
    rulerHour: {
      position: 'absolute',
      right: 6,
      fontSize: 12,
      color: c.textSecondary,
      // Nudge up so the number sits centred on its gridline.
      marginTop: -7,
    },
    canvas: { flex: 1, position: 'relative' },
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
      paddingVertical: chrome(3),
      paddingHorizontal: chrome(6),
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      overflow: 'hidden',
    },
    pressed: { opacity: 0.7 },
    muted: { fontSize: 15, color: c.textSecondary, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
  });
