import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import { nextPeriod, prevPeriod, today, VIEWS, type ViewId } from './viewMath';

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

interface PersistedView {
  view?: ViewId;
  anchor?: string;
}

interface ViewStateValue {
  view: ViewId;
  anchor: Date;
  setView: (v: ViewId) => void;
  setAnchor: (d: Date) => void;
  jumpToToday: () => void;
  goPrev: () => void;
  goNext: () => void;
}

const ViewStateContext = createContext<ViewStateValue | null>(null);

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
    if (initial.anchor) {
      const d = new Date(initial.anchor);
      if (!Number.isNaN(d.getTime())) return d;
    }
    return today();
  });

  useEffect(() => {
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ view, anchor: anchor.toISOString() }),
      );
    } catch {
      // Quota / private mode — non-fatal.
    }
  }, [view, anchor]);

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

  const value = useMemo<ViewStateValue>(
    () => ({ view, anchor, setView, setAnchor, jumpToToday, goPrev, goNext }),
    [view, anchor, setView, setAnchor, jumpToToday, goPrev, goNext],
  );

  return (
    <ViewStateContext.Provider value={value}>
      {children}
    </ViewStateContext.Provider>
  );
}

export function useViewState(): ViewStateValue {
  const ctx = useContext(ViewStateContext);
  if (!ctx) {
    throw new Error('useViewState must be used inside <ViewStateProvider>');
  }
  return ctx;
}

/**
 * Wire global keyboard shortcuts to the view state.
 *
 * - Ctrl/Cmd + 1..6 → switch view in the order from `viewMath.VIEWS`.
 * - Ctrl/Cmd + T   → jump to today.
 * - Ctrl/Cmd + ←/→ → previous/next period.
 *
 * Ignores keystrokes while the user is typing in a form control.
 */
export function useViewShortcuts(): void {
  const { setView, jumpToToday, goPrev, goNext } = useViewState();

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (isEditableTarget(e.target)) return;
      const cmd = e.ctrlKey || e.metaKey;
      if (!cmd) return;

      if (e.key >= '1' && e.key <= '6') {
        const index = Number(e.key) - 1;
        const next = VIEWS[index];
        if (next) {
          e.preventDefault();
          setView(next);
        }
        return;
      }

      const k = e.key.toLowerCase();
      if (k === 't') {
        e.preventDefault();
        jumpToToday();
        return;
      }
      if (e.key === 'ArrowLeft') {
        e.preventDefault();
        goPrev();
        return;
      }
      if (e.key === 'ArrowRight') {
        e.preventDefault();
        goNext();
        return;
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [setView, jumpToToday, goPrev, goNext]);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return true;
  if (target.isContentEditable) return true;
  return false;
}
