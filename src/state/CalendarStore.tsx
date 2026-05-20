import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import {
  listAccounts,
  listCalendars,
  listColorLabels,
  listTaskLists,
} from '../api/client';
import type { Account, Calendar, ColorLabel, TaskList } from '../api/types';

/**
 * Calendar and task-list store.
 *
 * One context holds the **set of containers** (calendars, task lists) and
 * which of them the user currently has visible. Individual data hooks
 * (`useEvents`, `useTasks`) live next door and consume the selection
 * from here.
 *
 * The selection is persisted to `localStorage` so it survives reloads.
 * Once the event-log sync is in place (Phase 7), this can move into the
 * settings sync — but the surface stays the same.
 */

const STORAGE_KEY = 'aperio.selection.v1';

interface PersistedSelection {
  calendars?: string[];
  taskLists?: string[];
}

interface CalendarStoreState {
  calendars: Calendar[];
  selectedCalendarIds: Set<string>;
  toggleCalendar: (id: string) => void;
  refreshCalendars: () => Promise<void>;

  taskLists: TaskList[];
  selectedTaskListIds: Set<string>;
  toggleTaskList: (id: string) => void;
  refreshTaskLists: () => Promise<void>;

  colorLabels: ColorLabel[];
  refreshColorLabels: () => Promise<void>;

  /** Accounts, used by the sidebar's tree view to group containers
   *  by their owning source. Refreshed alongside containers; the
   *  AccountsDialog also calls `refreshAccounts` after add/delete. */
  accounts: Account[];
  refreshAccounts: () => Promise<void>;

  loading: boolean;
}

const CalendarStoreContext = createContext<CalendarStoreState | null>(null);

function readPersisted(): PersistedSelection {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw);
  } catch {
    return {};
  }
}

function writePersisted(value: PersistedSelection) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  } catch {
    // Storage may be unavailable (private mode, quota); we can live without.
  }
}

export function CalendarStoreProvider({ children }: { children: ReactNode }) {
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [taskLists, setTaskLists] = useState<TaskList[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [selectedCalendarIds, setSelectedCalendarIds] = useState<Set<string>>(
    () => new Set(readPersisted().calendars ?? []),
  );
  const [selectedTaskListIds, setSelectedTaskListIds] = useState<Set<string>>(
    () => new Set(readPersisted().taskLists ?? []),
  );
  const [loading, setLoading] = useState(true);

  const refreshCalendars = useCallback(async () => {
    const list = await listCalendars();
    setCalendars(list);
    setSelectedCalendarIds((prev) => reconcileSelection(prev, list));
  }, []);

  const refreshTaskLists = useCallback(async () => {
    const list = await listTaskLists();
    setTaskLists(list);
    setSelectedTaskListIds((prev) => reconcileSelection(prev, list));
  }, []);

  const refreshColorLabels = useCallback(async () => {
    const labels = await listColorLabels();
    setColorLabels(labels);
  }, []);

  const refreshAccounts = useCallback(async () => {
    const list = await listAccounts();
    setAccounts(list);
  }, []);

  // Initial load: pull both lists in parallel, then drop the loading
  // flag. The store doesn't auto-refresh on dialog close (we don't yet
  // have one of our own — and creating containers happens through the
  // Sidebar, which calls refresh* directly). When the dialog system
  // grows a container-management dialog, it can call these helpers too.
  useEffect(() => {
    let cancelled = false;
    Promise.allSettled([
      refreshCalendars(),
      refreshTaskLists(),
      refreshColorLabels(),
      refreshAccounts(),
    ]).then(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [refreshCalendars, refreshTaskLists, refreshColorLabels, refreshAccounts]);

  // Persist any selection change.
  useEffect(() => {
    writePersisted({
      calendars: [...selectedCalendarIds],
      taskLists: [...selectedTaskListIds],
    });
  }, [selectedCalendarIds, selectedTaskListIds]);

  const toggleCalendar = useCallback((id: string) => {
    setSelectedCalendarIds((prev) => toggleSet(prev, id));
  }, []);

  const toggleTaskList = useCallback((id: string) => {
    setSelectedTaskListIds((prev) => toggleSet(prev, id));
  }, []);

  const value = useMemo<CalendarStoreState>(
    () => ({
      calendars,
      selectedCalendarIds,
      toggleCalendar,
      refreshCalendars,
      taskLists,
      selectedTaskListIds,
      toggleTaskList,
      refreshTaskLists,
      colorLabels,
      refreshColorLabels,
      accounts,
      refreshAccounts,
      loading,
    }),
    [
      calendars,
      selectedCalendarIds,
      toggleCalendar,
      refreshCalendars,
      taskLists,
      selectedTaskListIds,
      toggleTaskList,
      refreshTaskLists,
      colorLabels,
      refreshColorLabels,
      accounts,
      refreshAccounts,
      loading,
    ],
  );

  return (
    <CalendarStoreContext.Provider value={value}>
      {children}
    </CalendarStoreContext.Provider>
  );
}

export function useCalendarStore(): CalendarStoreState {
  const ctx = useContext(CalendarStoreContext);
  if (!ctx) {
    throw new Error('useCalendarStore must be used inside <CalendarStoreProvider>');
  }
  return ctx;
}

/**
 * Selection reconciliation: when the backing list changes, drop ids that
 * no longer exist and — if the previous selection was empty — default to
 * selecting *everything*. The second part matters for first-run, where
 * users expect their freshly created calendar to be visible without
 * having to tick a box.
 */
function reconcileSelection<T extends { id: string }>(
  prev: Set<string>,
  list: T[],
): Set<string> {
  const valid = new Set(list.map((x) => x.id));
  if (prev.size === 0) {
    return valid;
  }
  const next = new Set<string>();
  prev.forEach((id) => {
    if (valid.has(id)) next.add(id);
  });
  return next;
}

function toggleSet(prev: Set<string>, id: string): Set<string> {
  const next = new Set(prev);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  return next;
}
