import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import { CalendarStoreContext } from './calendarStoreContext';
import {
  reconcileSelectionTracked,
  type SelectionSlice,
} from './selectionReconcile';

import { sortSections } from '@aperio/shared';
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
  ContainerColor,
  Section,
  TaskList,
} from '../api/types';

/**
 * Resolve each container's bound color-label to its *live* hex, so the
 * whole app (sidebar, panels, views) sees a single derived `color`. When
 * a container is bound (`color_label`), its display color becomes the
 * label's current hex; recoloring the label re-derives here and every
 * consumer re-renders. Unbound containers keep their native color.
 *
 * Module-private (not a component) so it stays out of react-refresh's way.
 */
function withResolvedColors<
  T extends { color: ContainerColor | null; color_label: string | null },
>(containers: T[], byId: Map<string, ColorLabel>): T[] {
  return containers.map((c) => {
    if (!c.color_label) return c;
    const label = byId.get(c.color_label);
    if (!label) return c;
    return { ...c, color: { hex: label.hex, source: 'custom' as const } };
  });
}

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
  /** id → owning account, learned from every listing the id appeared
   *  in. Lets the reconciler tell a GENUINE removal (the owning
   *  account answered with content, this id is gone from it) apart
   *  from a COLD/FAILED listing (the account is absent from the
   *  listing entirely — snapshot not warmed yet, or a transient
   *  backend failure emptied it). Absent on older blobs; ids without
   *  an origin are treated conservatively (retained). */
  calendarOrigins?: Record<string, string>;
  taskListOrigins?: Record<string, string>;
  contactListOrigins?: Record<string, string>;
}

export interface CalendarStoreState {
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
  /** Each section's resolved *live* color hex, keyed by section id — the
   *  middle level of the task-color chain (task → section → list). A
   *  section with no bound label is absent. Re-derives when sections or
   *  color labels change. */
  sectionColorById: Map<string, string>;

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

  /** "Any catalog still doing its initial load" — the aggregate of the
   *  three per-type flags below. Kept for general consumers. */
  loading: boolean;
  /** Per-container-type initial-load flags. Each data hook gates on the
   *  ONE catalog it actually needs (events → calendars, tasks → task
   *  lists, contacts → contact lists) so a slow catalog from one source
   *  never blocks an unrelated view's first paint. This is what stops the
   *  task list from waiting seconds on a slow calendar enumeration. */
  calendarsLoading: boolean;
  taskListsLoading: boolean;
  contactListsLoading: boolean;
}


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
      origin: persisted.calendarOrigins ?? {},
    };
  });
  const [taskListSel, setTaskListSel] = useState<SelectionSlice>(() => {
    const persisted = readPersisted();
    return {
      selected: new Set(persisted.taskLists ?? []),
      known: persisted.knownTaskListIds
        ? new Set(persisted.knownTaskListIds)
        : null,
      origin: persisted.taskListOrigins ?? {},
    };
  });
  const [contactListSel, setContactListSel] = useState<SelectionSlice>(() => {
    const persisted = readPersisted();
    return {
      selected: new Set(persisted.contactLists ?? []),
      known: persisted.knownContactListIds
        ? new Set(persisted.knownContactListIds)
        : null,
      origin: persisted.contactListOrigins ?? {},
    };
  });
  // Per-source initial-load flags (see CalendarStoreState). Each flips
  // independently the moment its own catalog read returns, so a slow
  // provider on one container type can't gate another type's first paint.
  const [calendarsLoading, setCalendarsLoading] = useState(true);
  const [taskListsLoading, setTaskListsLoading] = useState(true);
  const [contactListsLoading, setContactListsLoading] = useState(true);
  const loading = calendarsLoading || taskListsLoading || contactListsLoading;

  // The account-id set rides along into the reconciler so containers of a
  // DELETED account are pruned from the persisted selection (a cold/failed
  // listing alone can't distinguish "account gone" from "account not warmed
  // yet"). Best-effort: on failure the reconciler just skips that pruning.
  const accountIdSet = useCallback(async () => {
    try {
      return new Set((await listAccounts()).map((a) => a.id));
    } catch {
      return null;
    }
  }, []);

  const refreshCalendars = useCallback(async () => {
    const [list, accountIds] = await Promise.all([
      listCalendars(),
      accountIdSet(),
    ]);
    setCalendars(list);
    setCalendarSel((prev) =>
      reconcileSelectionTracked(prev, list, undefined, accountIds),
    );
  }, [accountIdSet]);

  const refreshTaskLists = useCallback(async () => {
    const [list, accountIds] = await Promise.all([
      listTaskLists(),
      accountIdSet(),
    ]);
    setTaskLists(list);
    setTaskListSel((prev) =>
      reconcileSelectionTracked(prev, list, undefined, accountIds),
    );
  }, [accountIdSet]);

  const refreshContactLists = useCallback(async () => {
    const [list, accountIds] = await Promise.all([
      listContactLists(),
      accountIdSet(),
    ]);
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
      reconcileSelectionTracked(prev, list, (l) => !l.read_only, accountIds),
    );
  }, [accountIdSet]);

  const refreshColorLabels = useCallback(async () => {
    const labels = await listColorLabels();
    setColorLabels(labels);
  }, []);

  const refreshAccounts = useCallback(async () => {
    const list = await listAccounts();
    setAccounts(list);
  }, []);

  const loadSections = useCallback(async (listId: string) => {
    // Sorted HERE, once, rather than at each of the half-dozen places that
    // render `sectionsByList` — the task view, the backlog rail, the day and
    // month grids, the task editor, the section editor. A comparator repeated
    // per consumer is a comparator that ends up applied in five of six.
    const secs = sortSections(await getSections(listId));
    setSectionsByList((prev) => ({ ...prev, [listId]: secs }));
    return secs;
  }, []);

  // Initial load: pull every catalog in parallel and drop EACH type's
  // loading flag the moment its own read returns — not after the slowest
  // of all of them. That decoupling is the point: the backend serves each
  // catalog from its snapshot without blocking on the network (a cold
  // snapshot refreshes in the background), so a slow calendar source no
  // longer holds the task list (or contacts) hostage at startup. The store
  // doesn't auto-refresh on dialog close — container creation happens
  // through the Sidebar, which calls refresh* directly, and CacheSyncListener
  // re-runs these when a background catalog refresh lands.
  useEffect(() => {
    let cancelled = false;
    void refreshCalendars().finally(() => {
      if (!cancelled) setCalendarsLoading(false);
    });
    void refreshTaskLists().finally(() => {
      if (!cancelled) setTaskListsLoading(false);
    });
    void refreshContactLists().finally(() => {
      if (!cancelled) setContactListsLoading(false);
    });
    void refreshColorLabels();
    void refreshAccounts();
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
      calendarOrigins: calendarSel.origin,
      taskListOrigins: taskListSel.origin,
      contactListOrigins: contactListSel.origin,
    });
  }, [calendarSel, taskListSel, contactListSel]);

  const toggleCalendar = useCallback((id: string) => {
    setCalendarSel((prev) => ({
      ...prev,
      selected: toggleSet(prev.selected, id),
    }));
  }, []);

  const toggleTaskList = useCallback((id: string) => {
    setTaskListSel((prev) => ({
      ...prev,
      selected: toggleSet(prev.selected, id),
    }));
  }, []);

  const toggleContactList = useCallback((id: string) => {
    setContactListSel((prev) => ({
      ...prev,
      selected: toggleSet(prev.selected, id),
    }));
  }, []);

  // Containers carry a color-label BINDING (`color_label`); resolve it to
  // the label's live hex once, here, so every consumer sees the bound
  // color via the usual `container.color`. Recoloring a label re-derives.
  const labelsById = useMemo(() => {
    const m = new Map<string, ColorLabel>();
    colorLabels.forEach((l) => m.set(l.id, l));
    return m;
  }, [colorLabels]);
  const resolvedCalendars = useMemo(
    () => withResolvedColors(calendars, labelsById),
    [calendars, labelsById],
  );
  const resolvedTaskLists = useMemo(
    () => withResolvedColors(taskLists, labelsById),
    [taskLists, labelsById],
  );
  const resolvedContactLists = useMemo(
    () => withResolvedColors(contactLists, labelsById),
    [contactLists, labelsById],
  );

  // Section colors: each section's bound label resolved to its live hex.
  // Sections carry only a label binding (no native color), so we look the
  // label up directly. The middle step of the task-color chain.
  const sectionColorById = useMemo(() => {
    const m = new Map<string, string>();
    for (const sections of Object.values(sectionsByList)) {
      for (const section of sections) {
        if (section.color_label) {
          const label = labelsById.get(section.color_label);
          if (label) m.set(section.id, label.hex);
        }
      }
    }
    return m;
  }, [sectionsByList, labelsById]);

  const value = useMemo<CalendarStoreState>(
    () => ({
      calendars: resolvedCalendars,
      selectedCalendarIds: calendarSel.selected,
      toggleCalendar,
      refreshCalendars,
      taskLists: resolvedTaskLists,
      selectedTaskListIds: taskListSel.selected,
      toggleTaskList,
      refreshTaskLists,
      sectionsByList,
      loadSections,
      sectionColorById,
      contactLists: resolvedContactLists,
      selectedContactListIds: contactListSel.selected,
      toggleContactList,
      refreshContactLists,
      colorLabels,
      refreshColorLabels,
      accounts,
      refreshAccounts,
      loading,
      calendarsLoading,
      taskListsLoading,
      contactListsLoading,
    }),
    [
      resolvedCalendars,
      calendarSel.selected,
      toggleCalendar,
      refreshCalendars,
      resolvedTaskLists,
      taskListSel.selected,
      toggleTaskList,
      refreshTaskLists,
      sectionsByList,
      loadSections,
      sectionColorById,
      resolvedContactLists,
      contactListSel.selected,
      toggleContactList,
      refreshContactLists,
      colorLabels,
      refreshColorLabels,
      accounts,
      refreshAccounts,
      loading,
      calendarsLoading,
      taskListsLoading,
      contactListsLoading,
    ],
  );

  return (
    <CalendarStoreContext.Provider value={value}>
      {children}
    </CalendarStoreContext.Provider>
  );
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
