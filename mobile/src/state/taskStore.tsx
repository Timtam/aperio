import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';

import type { ColorLabel, Section, TaskList } from '@aperio/shared';

import { listColorLabels } from '../api/colorLabels';
import { getSections, listTaskLists } from '../api/client';
import { TaskStoreContext } from './taskStoreContext';
import {
  reconcileSelectionTracked,
  STORAGE_KEY,
  toggleSet,
  type PersistedSelection,
  type SelectionSlice,
} from './selection';

/**
 * The mobile task store — the single context every task screen consumes.
 *
 * Collapses the desktop's two pieces into one: the `CalendarStore` task slice
 * (catalog + selection Set + sections cache) AND the `DialogState`
 * `dataVersion` invalidation counter. The desktop keeps `dataVersion` in the
 * dialog stack because that's where mutations close; mobile has navigation
 * instead of a dialog stack, so the counter lives here and the navigation /
 * view layers bump it the same way the desktop's `close()` did.
 */
export interface TaskStoreState {
  taskLists: TaskList[];
  selectedTaskListIds: Set<string>;
  toggleTaskList: (id: string) => void;
  refreshTaskLists: () => Promise<void>;
  /** Cached sections per list id, populated lazily by `loadSections`. Empty in
   *  2b (the flat list doesn't group yet); the grouped screen (sub-3) fills it. */
  sectionsByList: Record<string, Section[]>;
  loadSections: (listId: string) => Promise<Section[]>;
  /** The app-wide colour-label palette (named + ad-hoc), so editors can offer a
   *  picker and rows can resolve a `color_label` id → its name. */
  colorLabels: ColorLabel[];
  refreshColorLabels: () => Promise<void>;
  taskListsLoading: boolean;
  /** Bumped on every mutation; `useTasks` keys its cache on it (full refetch). */
  dataVersion: number;
  invalidateData: () => void;
}

async function readPersisted(): Promise<PersistedSelection> {
  try {
    const raw = await AsyncStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as PersistedSelection) : {};
  } catch {
    return {};
  }
}

async function writePersisted(value: PersistedSelection): Promise<void> {
  try {
    await AsyncStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  } catch {
    // Storage may be unavailable; the app works without persisted selection.
  }
}

export function TaskStoreProvider({ children }: { children: ReactNode }) {
  const [taskLists, setTaskLists] = useState<TaskList[]>([]);
  const [taskListSel, setTaskListSel] = useState<SelectionSlice>({
    selected: new Set(),
    known: null,
  });
  const [sectionsByList, setSectionsByList] = useState<
    Record<string, Section[]>
  >({});
  const [taskListsLoading, setTaskListsLoading] = useState(true);
  const [colorLabels, setColorLabels] = useState<ColorLabel[]>([]);
  const [dataVersion, setDataVersion] = useState(0);

  // Selection persistence is hydrated asynchronously (AsyncStorage), unlike the
  // desktop's synchronous localStorage. Until the stored blob has loaded we
  // must neither persist (which would clobber it with the empty initial state)
  // nor run the initial reconcile (which would first-run-select everything).
  const hydrated = useRef(false);

  const invalidateData = useCallback(() => setDataVersion((v) => v + 1), []);

  const refreshTaskLists = useCallback(async () => {
    const list = await listTaskLists();
    setTaskLists(list);
    setTaskListSel((prev) => reconcileSelectionTracked(prev, list));
  }, []);

  const loadSections = useCallback(async (listId: string) => {
    const secs = await getSections(listId);
    setSectionsByList((prev) => ({ ...prev, [listId]: secs }));
    return secs;
  }, []);

  const refreshColorLabels = useCallback(async () => {
    setColorLabels(await listColorLabels());
  }, []);

  const toggleTaskList = useCallback((id: string) => {
    setTaskListSel((prev) => ({
      selected: toggleSet(prev.selected, id),
      known: prev.known,
    }));
  }, []);

  // Hydrate the persisted selection, THEN run the initial catalog load so the
  // reconciler sees the restored `prev` (faithful to the desktop's synchronous
  // read at init). Queued functional `setState`s apply in order, so the
  // reconcile's `prev` is the hydrated slice even though both updates land
  // before the next render.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const persisted = await readPersisted();
      if (cancelled) return;
      setTaskListSel({
        selected: new Set(persisted.taskLists ?? []),
        known: persisted.knownTaskListIds
          ? new Set(persisted.knownTaskListIds)
          : null,
      });
      hydrated.current = true;
      try {
        await refreshTaskLists();
        // Best-effort — the palette is non-critical; a failure just means rows
        // render without colour-label names until the next refresh.
        await refreshColorLabels().catch(() => {});
      } finally {
        if (!cancelled) setTaskListsLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [refreshTaskLists, refreshColorLabels]);

  // Persist selection + known together, but only after hydration so the empty
  // initial state never freezes over real stored data.
  useEffect(() => {
    if (!hydrated.current) return;
    void writePersisted({
      taskLists: [...taskListSel.selected],
      knownTaskListIds: taskListSel.known ? [...taskListSel.known] : undefined,
    });
  }, [taskListSel]);

  const value = useMemo<TaskStoreState>(
    () => ({
      taskLists,
      selectedTaskListIds: taskListSel.selected,
      toggleTaskList,
      refreshTaskLists,
      sectionsByList,
      loadSections,
      colorLabels,
      refreshColorLabels,
      taskListsLoading,
      dataVersion,
      invalidateData,
    }),
    [
      taskLists,
      taskListSel.selected,
      toggleTaskList,
      refreshTaskLists,
      sectionsByList,
      loadSections,
      colorLabels,
      refreshColorLabels,
      taskListsLoading,
      dataVersion,
      invalidateData,
    ],
  );

  return (
    <TaskStoreContext.Provider value={value}>
      {children}
    </TaskStoreContext.Provider>
  );
}
