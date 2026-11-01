import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import { getUserPref, setUserPref } from '../api/client';
import {
  nextPeriod,
  prevPeriod,
  today,
  VIEWS,
  type ViewId,
  type WeekStart,
  DEFAULT_TIME_STEP,
  isValidTimeStep,
  type TimeStepMinutes,
} from './viewMath';
import { useUserPrefsChanged } from './useUserPrefsChanged';
import { ViewStateContext, type ViewStateValue } from './viewStateContext';

/**
 * Active-view state + navigation shortcuts.
 *
 * - `view` — the active calendar/task view.
 * - `anchor` — the date the view is centred on (the focused day in
 *   WeekView, the visible month in MonthView, etc.).
 * - `jumpToToday`, `prevPeriod`, `nextPeriod`, `setView`, `setAnchor` —
 *   intent-level navigation. The shortcut layer (`useViewShortcuts`)
 *   binds Ctrl+1..6, Ctrl+T, Ctrl+Left/Right to them.
 *
 * The view and anchor persist in `localStorage` so the app reopens where
 * the user left it. Anchor persistence intentionally only stores the
 * date — never reusing yesterday's day at midnight surprises nobody.
 */

const STORAGE_KEY = 'aperio.view.v1';
/** Synced (cross-device) pref for the visual first day of the week. Unlike
 *  `view`/`anchor` (device-local localStorage), this rides the sync log. */
const WEEK_START_PREF = 'view.weekStart';
/** Synced pref: seed every view to today on launch instead of restoring the
 *  last-opened day. Mirrored into the local `PersistedView` blob so the
 *  launch decision is available SYNCHRONOUSLY (before the async pref hydrate)
 *  — otherwise the view would flash yesterday's day then jump to today. */
const START_ON_TODAY_PREF = 'view.startOnToday';
/** Synced pref: show cancelled events in the calendar (default on, for Outlook
 *  consistency) or hide them. Reminders for cancelled events are always
 *  suppressed regardless of this toggle. */
const SHOW_CANCELLED_PREF = 'view.showCancelledEvents';
/** Synced pref: show and announce calendar weeks (default on). Off hides the
 *  number from every view and the toolbar and keeps it out of what the screen
 *  reader hears; the K key still says the week on request. */
const WEEK_NUMBERS_PREF = 'view.weekNumbers';
/** Synced prefs: whether HIDDEN (deselected) but writable calendars / task lists
 *  are still offered as assignment targets in the editors + move/copy pickers
 *  (default on). Off = only currently-visible containers are pickable. */
const SHOW_HIDDEN_CALENDAR_TARGETS_PREF = 'pickers.showHiddenCalendarTargets';
const SHOW_HIDDEN_TASK_LIST_TARGETS_PREF = 'pickers.showHiddenTaskListTargets';
/** Synced pref: how far one press of the time field's minute spinner moves.
 *  Minute-by-minute is the browser default and it is a lot of presses for a
 *  half-past-nine meeting; Outlook steps in 5, Google in 15. */
const TIME_STEP_PREF = 'editor.timeStepMinutes';
/** Every synced pref this provider reads, at mount and again when a sync
 *  round changes it. */
const OWNED_PREFS = [
  WEEK_START_PREF,
  START_ON_TODAY_PREF,
  SHOW_CANCELLED_PREF,
  WEEK_NUMBERS_PREF,
  SHOW_HIDDEN_CALENDAR_TARGETS_PREF,
  SHOW_HIDDEN_TASK_LIST_TARGETS_PREF,
  TIME_STEP_PREF,
] as const;


function isValidWeekStart(n: number): n is WeekStart {
  return Number.isInteger(n) && n >= 0 && n <= 6;
}

interface PersistedView {
  view?: ViewId;
  anchor?: string;
  focusedCalendarId?: string | null;
  /** Local mirror of the synced `view.startOnToday` pref (see above). */
  startOnToday?: boolean;
}

function readPersisted(): PersistedView {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as PersistedView;
  } catch {
    return {};
  }
}

function isValidView(v: unknown): v is ViewId {
  return typeof v === 'string' && (VIEWS as readonly string[]).includes(v);
}

export function ViewStateProvider({ children }: { children: ReactNode }) {
  const initial = readPersisted();
  const [view, setViewState] = useState<ViewId>(() =>
    isValidView(initial.view) ? initial.view : 'week',
  );
  const [anchor, setAnchorState] = useState<Date>(() => {
    // "Start on today" (synced pref, mirrored locally) overrides the restored
    // day at launch — every view opens on today. Otherwise restore the
    // last-opened day.
    if (!initial.startOnToday && initial.anchor) {
      const d = new Date(initial.anchor);
      if (!Number.isNaN(d.getTime())) return d;
    }
    return today();
  });
  const [startOnToday, setStartOnTodayState] = useState<boolean>(
    () => initial.startOnToday ?? false,
  );
  const [focusedCalendarId, setFocusedCalendarId] = useState<string | null>(
    () =>
      typeof initial.focusedCalendarId === 'string'
        ? initial.focusedCalendarId
        : null,
  );
  // Synced view pref (cross-device), hydrated from user_prefs on mount.
  // Defaults to Monday (ISO) until the round-trip returns.
  const [weekStartsOn, setWeekStartsOnState] = useState<WeekStart>(1);
  // Synced pref: show cancelled events (default on). Hidden events are filtered
  // out in `useEvents`; reminders for them are always suppressed core-side.
  const [showCancelledEvents, setShowCancelledEventsState] =
    useState<boolean>(true);
  // Synced pref: show and announce calendar weeks (default on).
  const [weekNumbers, setWeekNumbersState] = useState<boolean>(true);
  // Synced prefs: offer hidden (deselected) writable containers as targets in
  // the pickers (default on until the round-trip returns).
  const [showHiddenCalendarTargets, setShowHiddenCalendarTargetsState] =
    useState<boolean>(true);
  const [showHiddenTaskListTargets, setShowHiddenTaskListTargetsState] =
    useState<boolean>(true);
  const [timeStepMinutes, setTimeStepMinutesState] =
    useState<TimeStepMinutes>(DEFAULT_TIME_STEP);
  // Read the synced prefs this provider owns: all of them at mount, and the
  // ones a sync round changed afterwards, so a choice made on another device
  // applies here without a restart. Each keeps its default until its answer
  // arrives, and keeps it when the backend is unreachable.
  const readPrefs = useCallback(
    (keys: readonly string[], alive: () => boolean = () => true) => {
      const read = (key: string, apply: (raw: string) => void) => {
        if (!keys.includes(key)) return;
        getUserPref(key)
          .then((raw) => {
            if (alive() && raw != null) apply(raw);
          })
          .catch(() => {});
      };
      read(WEEK_START_PREF, (raw) => {
        const n = Number(raw);
        if (isValidWeekStart(n)) setWeekStartsOnState(n);
      });
      read(SHOW_CANCELLED_PREF, (raw) =>
        setShowCancelledEventsState(raw !== 'false'),
      );
      read(WEEK_NUMBERS_PREF, (raw) => setWeekNumbersState(raw !== 'false'));
      read(TIME_STEP_PREF, (raw) => {
        const n = Number(raw);
        if (isValidTimeStep(n)) setTimeStepMinutesState(n);
      });
      read(SHOW_HIDDEN_CALENDAR_TARGETS_PREF, (raw) =>
        setShowHiddenCalendarTargetsState(raw !== 'false'),
      );
      read(SHOW_HIDDEN_TASK_LIST_TARGETS_PREF, (raw) =>
        setShowHiddenTaskListTargetsState(raw !== 'false'),
      );
      // Start-on-today only refreshes the toggle state + the local mirror
      // (written back by the persist effect) for the NEXT launch — it
      // deliberately does NOT move the current anchor, so a sync arriving
      // mid-session can't yank the day the user is looking at.
      read(START_ON_TODAY_PREF, (raw) => setStartOnTodayState(raw === 'true'));
    },
    [],
  );
  useEffect(() => {
    let cancelled = false;
    readPrefs(OWNED_PREFS, () => !cancelled);
    return () => {
      cancelled = true;
    };
  }, [readPrefs]);
  useUserPrefsChanged(OWNED_PREFS, (changed) => readPrefs(changed));

  useEffect(() => {
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({
          view,
          anchor: anchor.toISOString(),
          focusedCalendarId,
          startOnToday,
        }),
      );
    } catch {
      // Quota / private mode — non-fatal.
    }
  }, [view, anchor, focusedCalendarId, startOnToday]);

  // Counts the user's requests for another period (Back, Today, Forward,
  // Ctrl+←/→, Ctrl+T), so a view can say the period it now shows.
  const [pageTurn, setPageTurn] = useState(0);
  const setView = useCallback((v: ViewId) => setViewState(v), []);
  const setAnchor = useCallback((d: Date) => setAnchorState(d), []);
  const jumpToToday = useCallback(() => {
    setAnchorState(today());
    setPageTurn((n) => n + 1);
  }, []);
  const goPrev = useCallback(() => {
    setAnchorState((d) => prevPeriod(view, d));
    setPageTurn((n) => n + 1);
  }, [view]);
  const goNext = useCallback(() => {
    setAnchorState((d) => nextPeriod(view, d));
    setPageTurn((n) => n + 1);
  }, [view]);
  const enterFocus = useCallback(
    (calendarId: string) => setFocusedCalendarId(calendarId),
    [],
  );
  const exitFocus = useCallback(() => setFocusedCalendarId(null), []);
  const setWeekStartsOn = useCallback((d: WeekStart) => {
    setWeekStartsOnState(d);
    void setUserPref(WEEK_START_PREF, String(d));
  }, []);
  const setStartOnToday = useCallback((v: boolean) => {
    setStartOnTodayState(v);
    void setUserPref(START_ON_TODAY_PREF, v ? 'true' : 'false');
  }, []);
  const setShowCancelledEvents = useCallback((v: boolean) => {
    setShowCancelledEventsState(v);
    void setUserPref(SHOW_CANCELLED_PREF, v ? 'true' : 'false');
  }, []);
  const setWeekNumbers = useCallback((v: boolean) => {
    setWeekNumbersState(v);
    void setUserPref(WEEK_NUMBERS_PREF, v ? 'true' : 'false');
  }, []);
  const setTimeStepMinutes = useCallback((v: TimeStepMinutes) => {
    setTimeStepMinutesState(v);
    void setUserPref(TIME_STEP_PREF, String(v));
  }, []);
  const setShowHiddenCalendarTargets = useCallback((v: boolean) => {
    setShowHiddenCalendarTargetsState(v);
    void setUserPref(SHOW_HIDDEN_CALENDAR_TARGETS_PREF, v ? 'true' : 'false');
  }, []);
  const setShowHiddenTaskListTargets = useCallback((v: boolean) => {
    setShowHiddenTaskListTargetsState(v);
    void setUserPref(SHOW_HIDDEN_TASK_LIST_TARGETS_PREF, v ? 'true' : 'false');
  }, []);

  const value = useMemo<ViewStateValue>(
    () => ({
      view,
      anchor,
      pageTurn,
      setView,
      setAnchor,
      jumpToToday,
      goPrev,
      goNext,
      focusedCalendarId,
      enterFocus,
      exitFocus,
      weekStartsOn,
      setWeekStartsOn,
      startOnToday,
      setStartOnToday,
      showCancelledEvents,
      setShowCancelledEvents,
      weekNumbers,
      setWeekNumbers,
      showHiddenCalendarTargets,
      timeStepMinutes,
      setTimeStepMinutes,
      setShowHiddenCalendarTargets,
      showHiddenTaskListTargets,
      setShowHiddenTaskListTargets,
    }),
    [
      view,
      anchor,
      pageTurn,
      setView,
      setAnchor,
      jumpToToday,
      goPrev,
      goNext,
      focusedCalendarId,
      enterFocus,
      exitFocus,
      weekStartsOn,
      setWeekStartsOn,
      startOnToday,
      setStartOnToday,
      showCancelledEvents,
      setShowCancelledEvents,
      weekNumbers,
      setWeekNumbers,
      showHiddenCalendarTargets,
      timeStepMinutes,
      setTimeStepMinutes,
      setShowHiddenCalendarTargets,
      showHiddenTaskListTargets,
      setShowHiddenTaskListTargets,
    ],
  );

  return (
    <ViewStateContext.Provider value={value}>
      {children}
    </ViewStateContext.Provider>
  );
}

