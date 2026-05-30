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
  getSections,
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
  Section,
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
  /** Mirror sets for the three container types: every id the
   *  reconciler has ever seen for this user. Needed to tell
   *  "user actively unticked this" (in known, not in selection)
   *  apart from "freshly appeared, never seen before" (not in
   *  known) — the latter gets auto-selected so a newly-added
   *  calendar shows up in the sidebar without a ticking step.
   *  Absent on persisted blobs minted before this field landed;
   *  the reconciler handles that migration in one shot. */
  knownCalendarIds?: string[];
  knownTaskListIds?: string[];
  knownContactListIds?: string[];
}

/** Per-container-type slice tracked together so the reconciler
 *  can atomically update both the user's selection and the set of
 *  ids it's seen before. `known === null` means "never reconciled
 *  yet" — either first run or a localStorage blob from before
 *  this field existed. */
interface SelectionSlice {
  selected: Set<string>;
  known: Set<string> | null;
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

  /** Cached sections per list id (Vikunja buckets / Todoist sections /
   *  local sections). Populated lazily by `loadSections` — the task
   *  panel calls it for the list it's showing. Section-less lists cache
   *  an empty array, so a present-but-empty entry means "fetched, none".*/
  sectionsByList: Record<string, Section[]>;
  /** Fetch (and cache) the sections of one list. Returns them so a
   *  caller can use the result without waiting for the state update. */
  loadSections: (listId: string) => Promise<Section[]>;

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
  const [sectionsByList, setSectionsByList] = useState<
    Record<string, Section[]>
  >({});
  const [calendarSel, setCalendarSel] = useState<SelectionSlice>(() => {
    const persisted = readPersisted();
    return {
      selected: new Set(persisted.calendars ?? []),
      known: persisted.knownCalendarIds
        ? new Set(persisted.knownCalendarIds)
        : null,
    };
  });
  const [taskListSel, setTaskListSel] = useState<SelectionSlice>(() => {
    const persisted = readPersisted();
    return {
      selected: new Set(persisted.taskLists ?? []),
      known: persisted.knownTaskListIds
        ? new Set(persisted.knownTaskListIds)
        : null,
    };
  });
  const [contactListSel, setContactListSel] = useState<SelectionSlice>(() => {
    const persisted = readPersisted();
    return {
      selected: new Set(persisted.contactLists ?? []),
      known: persisted.knownContactListIds
        ? new Set(persisted.knownContactListIds)
        : null,
    };
  });
  const [loading, setLoading] = useState(true);

  const refreshCalendars = useCallback(async () => {
    const list = await listCalendars();
    setCalendars(list);
    setCalendarSel((prev) => reconcileSelectionTracked(prev, list));
  }, []);

  const refreshTaskLists = useCallback(async () => {
    const list = await listTaskLists();
    setTaskLists(list);
    setTaskListSel((prev) => reconcileSelectionTracked(prev, list));
  }, []);

  const refreshContactLists = useCallback(async () => {
    const list = await listContactLists();
    setContactLists(list);
    // Contact lists have a `read_only` flag; on first run AND for
    // newly-discovered lists the reconciler defaults to "writable
    // gets selected, read-only stays opt-in" so a fresh user sees
    // their personal address books without having to tick anything,
    // and a newly-added writable list pops in automatically. The
    // EWS Global Address List (a big read-only list — 1000+
    // entries via a slow ResolveNames walk) is the reason
    // read-only is opt-in: enabling it should be a deliberate
    // act, not a silent default.
    setContactListSel((prev) =>
      reconcileSelectionTracked(prev, list, (l) => !l.read_only),
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

  const loadSections = useCallback(async (listId: string) => {
    const secs = await getSections(listId);
    setSectionsByList((prev) => ({ ...prev, [listId]: secs }));
    return secs;
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

  // Persist selection + known sets together. Known only gets
  // written once it's no longer null (i.e. after the first
  // reconcile has happened) so we don't accidentally freeze a
  // half-loaded snapshot from a midflight initial fetch.
  useEffect(() => {
    writePersisted({
      calendars: [...calendarSel.selected],
      taskLists: [...taskListSel.selected],
      contactLists: [...contactListSel.selected],
      knownCalendarIds: calendarSel.known
        ? [...calendarSel.known]
        : undefined,
      knownTaskListIds: taskListSel.known
        ? [...taskListSel.known]
        : undefined,
      knownContactListIds: contactListSel.known
        ? [...contactListSel.known]
        : undefined,
    });
  }, [calendarSel, taskListSel, contactListSel]);

  const toggleCalendar = useCallback((id: string) => {
    setCalendarSel((prev) => ({
      selected: toggleSet(prev.selected, id),
      known: prev.known,
    }));
  }, []);

  const toggleTaskList = useCallback((id: string) => {
    setTaskListSel((prev) => ({
      selected: toggleSet(prev.selected, id),
      known: prev.known,
    }));
  }, []);

  const toggleContactList = useCallback((id: string) => {
    setContactListSel((prev) => ({
      selected: toggleSet(prev.selected, id),
      known: prev.known,
    }));
  }, []);

  const value = useMemo<CalendarStoreState>(
    () => ({
      calendars,
      selectedCalendarIds: calendarSel.selected,
      toggleCalendar,
      refreshCalendars,
      taskLists,
      selectedTaskListIds: taskListSel.selected,
      toggleTaskList,
      refreshTaskLists,
      sectionsByList,
      loadSections,
      contactLists,
      selectedContactListIds: contactListSel.selected,
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
      calendarSel.selected,
      toggleCalendar,
      refreshCalendars,
      taskLists,
      taskListSel.selected,
      toggleTaskList,
      refreshTaskLists,
      sectionsByList,
      loadSections,
      contactLists,
      contactListSel.selected,
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
 * Selection reconciliation against the latest list — runs every time
 * the backing list refreshes. Three distinct cases:
 *
 *   1. **First-ever run** (`prev.selected` empty AND `prev.known` null):
 *      default to selecting every item that passes `autoSelectNew`
 *      (or all items if no filter given). This is the first-run UX
 *      where the user expects their freshly created calendar to be
 *      visible without ticking a box.
 *
 *   2. **Existing user upgrade** (`prev.selected` populated AND
 *      `prev.known` null): localStorage from before known-tracking
 *      existed. Freeze `known := selected ∪ list-ids` so we don't
 *      surprise-select calendars the user had silently unticked
 *      under the old "empty means select-everything" reconciler.
 *      Their current selection stays exactly as it was.
 *
 *   3. **Steady state** (`prev.known` non-null): keep selected ids
 *      that still exist; auto-select any id we have NEVER seen
 *      before (passing `autoSelectNew`); leave already-known-but-
 *      unselected ids alone — those are ones the user explicitly
 *      unticked at some point. This is the fix for the "added a
 *      new calendar, it stayed unchecked" bug: a truly new id
 *      isn't in `known`, so it goes into `selected` by default.
 *
 * `autoSelectNew` lets the caller veto the auto-select default per
 * item — used by the contact-list reconciler to keep heavy
 * read-only lists (the EWS GAL) opt-in.
 *
 * Returned `known` always covers exactly the current list (dropping
 * ids that disappeared, adding the freshly-arrived ones) so the
 * next reconcile has a clean baseline.
 */
function reconcileSelectionTracked<T extends { id: string }>(
  prev: SelectionSlice,
  list: T[],
  autoSelectNew?: (item: T) => boolean,
): SelectionSlice {
  const listIds = new Set(list.map((x) => x.id));
  const isNewDefaultOn = (item: T) =>
    autoSelectNew ? autoSelectNew(item) : true;

  // Case 1: first-ever run.
  if (prev.selected.size === 0 && prev.known === null) {
    return {
      selected: new Set(list.filter(isNewDefaultOn).map((x) => x.id)),
      known: new Set(listIds),
    };
  }

  // Case 2: upgrade from pre-known-tracking localStorage. Freeze
  // known to "everything we know about right now" — both the
  // currently-selected and the currently-visible. Subsequent
  // reconciles will treat anything else as truly new.
  if (prev.known === null) {
    const selected = new Set<string>();
    prev.selected.forEach((id) => {
      if (listIds.has(id)) selected.add(id);
    });
    const known = new Set<string>(prev.selected);
    listIds.forEach((id) => known.add(id));
    return { selected, known };
  }

  // Case 3: steady state.
  const selected = new Set<string>();
  prev.selected.forEach((id) => {
    if (listIds.has(id)) selected.add(id);
  });
  for (const item of list) {
    if (!prev.known.has(item.id) && isNewDefaultOn(item)) {
      selected.add(item.id);
    }
  }
  // Trim known to current list + record any newly-seen ids.
  const known = new Set<string>();
  prev.known.forEach((id) => {
    if (listIds.has(id)) known.add(id);
  });
  listIds.forEach((id) => known.add(id));
  return { selected, known };
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

/** One node of the task-list project tree. */
export interface TaskListNode {
  list: TaskList;
  children: TaskListNode[];
  /** 0 for roots, +1 per level — the sidebar uses it for indentation. */
  depth: number;
}

/**
 * Build a parent→children forest from a flat task-list array using
 * `parent_id`. A list whose `parent_id` isn't present in `lists`
 * (different account, or a parent the user can't see) is promoted to a
 * root so nothing is dropped. Children preserve input order; flat
 * backends — where every `parent_id` is `null` — produce a depth-0
 * forest, exactly the pre-nesting shape.
 *
 * Callers pass an account-scoped subset (parent_id only ever refers to
 * a same-account list), so the forest never crosses account boundaries.
 */
export function buildTaskListForest(lists: TaskList[]): TaskListNode[] {
  const byId = new Map(lists.map((l) => [l.id, l]));
  const childrenOf = new Map<string, TaskList[]>();
  const roots: TaskList[] = [];
  for (const list of lists) {
    const parent = list.parent_id;
    if (parent && parent !== list.id && byId.has(parent)) {
      const arr = childrenOf.get(parent);
      if (arr) arr.push(list);
      else childrenOf.set(parent, [list]);
    } else {
      roots.push(list);
    }
  }
  const seen = new Set<string>();
  const build = (list: TaskList, depth: number): TaskListNode => {
    seen.add(list.id);
    const kids = (childrenOf.get(list.id) ?? []).filter((c) => !seen.has(c.id));
    return { list, depth, children: kids.map((c) => build(c, depth + 1)) };
  };
  const forest = roots.map((r) => build(r, 0));
  // Safety net: a list trapped in a parent cycle (corrupt data) is
  // never reached from a root — surface it as a depth-0 node rather
  // than dropping it silently.
  for (const list of lists) {
    if (!seen.has(list.id)) forest.push(build(list, 0));
  }
  return forest;
}
