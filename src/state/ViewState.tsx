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
} from './viewMath';
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
  useEffect(() => {
    let cancelled = false;
    getUserPref(WEEK_START_PREF)
      .then((raw) => {
        if (cancelled || raw == null) return;
        const n = Number(raw);
        if (isValidWeekStart(n)) setWeekStartsOnState(n);
      })
      .catch(() => {
        // Backend unreachable during init → keep the Monday default.
      });
    // Hydrate the show-cancelled pref (default on until the round-trip returns).
    getUserPref(SHOW_CANCELLED_PREF)
      .then((raw) => {
        if (cancelled || raw == null) return;
        setShowCancelledEventsState(raw !== 'false');
      })
      .catch(() => {
        // Backend unreachable → keep the default (show).
      });
    // Hydrate the synced start-on-today pref. This only refreshes the toggle
    // state + the local mirror (written back by the persist effect) for the
    // NEXT launch — it deliberately does NOT move the current anchor, so a
    // sync arriving mid-session can't yank the day the user is looking at.
    getUserPref(START_ON_TODAY_PREF)
      .then((raw) => {
        if (cancelled || raw == null) return;
        setStartOnTodayState(raw === 'true');
      })
      .catch(() => {
        // Backend unreachable → keep the synchronous local-mirror value.
      });
    return () => {
      cancelled = true;
    };
  }, []);

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

  const setView = useCallback((v: ViewId) => setViewState(v), []);
  const setAnchor = useCallback((d: Date) => setAnchorState(d), []);
  const jumpToToday = useCallback(() => setAnchorState(today()), []);
  const goPrev = useCallback(
    () => setAnchorState((d) => prevPeriod(view, d)),
    [view],
  );
  const goNext = useCallback(
    () => setAnchorState((d) => nextPeriod(view, d)),
    [view],
  );
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

  const value = useMemo<ViewStateValue>(
    () => ({
      view,
      anchor,
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
    }),
    [
      view,
      anchor,
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
    ],
  );

  return (
    <ViewStateContext.Provider value={value}>
      {children}
    </ViewStateContext.Provider>
  );
}

