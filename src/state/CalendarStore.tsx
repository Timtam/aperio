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
  listContactLists,
  listTaskLists,
} from '../api/client';
import type {
  Account,
  Calendar,
  ColorLabel,
  ContactList,
  TaskList,
} from '../api/types';

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
  contactLists?: string[];
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

  /** Address books (DESIGN.md §10). Each list has its own
   *  selection toggle so users can exclude e.g. the big read-only
   *  Global Address List from the contacts panel listing without
   *  hiding their personal books. */
  contactLists: ContactList[];
  selectedContactListIds: Set<string>;
  toggleContactList: (id: string) => void;
  refreshContactLists: () => Promise<void>;

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
  const [contactLists, setContactLists] = useState<ContactList[]>([]);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [selectedCalendarIds, setSelectedCalendarIds] = useState<Set<string>>(
    () => new Set(readPersisted().calendars ?? []),
  );
  const [selectedTaskListIds, setSelectedTaskListIds] = useState<Set<string>>(
    () => new Set(readPersisted().taskLists ?? []),
  );
  const [selectedContactListIds, setSelectedContactListIds] = useState<
    Set<string>
  >(() => new Set(readPersisted().contactLists ?? []));
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

  const refreshContactLists = useCallback(async () => {
    const list = await listContactLists();
    setContactLists(list);
    // Contact lists have a `read_only` flag; on first run the
    // reconciler defaults to "every writable list selected" so
    // a fresh user sees their personal address books without
    // having to tick anything. Read-only lists (the EWS Global
    // Address List is the big one — 1000+ entries via a slow
    // ResolveNames walk) are opt-in: the user enables them
    // explicitly via the sidebar checkbox once, and the
    // selection persists from then on.
    setSelectedContactListIds((prev) =>
      reconcileContactSelection(prev, list),
    );
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
      refreshContactLists(),
      refreshColorLabels(),
      refreshAccounts(),
    ]).then(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [
    refreshCalendars,
    refreshTaskLists,
    refreshContactLists,
    refreshColorLabels,
    refreshAccounts,
  ]);

  // Persist any selection change.
  useEffect(() => {
    writePersisted({
      calendars: [...selectedCalendarIds],
      taskLists: [...selectedTaskListIds],
      contactLists: [...selectedContactListIds],
    });
  }, [selectedCalendarIds, selectedTaskListIds, selectedContactListIds]);

  const toggleCalendar = useCallback((id: string) => {
    setSelectedCalendarIds((prev) => toggleSet(prev, id));
  }, []);

  const toggleTaskList = useCallback((id: string) => {
    setSelectedTaskListIds((prev) => toggleSet(prev, id));
  }, []);

  const toggleContactList = useCallback((id: string) => {
    setSelectedContactListIds((prev) => toggleSet(prev, id));
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
      contactLists,
      selectedContactListIds,
      toggleContactList,
      refreshContactLists,
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
      contactLists,
      selectedContactListIds,
      toggleContactList,
      refreshContactLists,
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

/**
 * Variant of `reconcileSelection` that respects the `read_only`
 * flag when defaulting first-run selection. Read-only lists
 * (the EWS GAL) are heavy to enumerate, so they're opt-in
 * rather than auto-selected. After the user has manually
 * enabled one, it's persisted like any other and the regular
 * "remove ids that no longer exist" pass takes over.
 */
function reconcileContactSelection(
  prev: Set<string>,
  list: ContactList[],
): Set<string> {
  if (prev.size === 0) {
    return new Set(list.filter((l) => !l.read_only).map((l) => l.id));
  }
  const valid = new Set(list.map((l) => l.id));
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
