// Selection reconciliation — lifted verbatim from the desktop's
// `CalendarStore.tsx` (the task-list slice). Pure functions, no React, so the
// store can use them and tests can exercise them directly. Keeping the logic
// byte-for-byte identical to the desktop is deliberate: the "auto-select a
// freshly-appeared list, but never re-select one the user unticked" semantics
// must match exactly.
//
// On mobile the persisted blob lives in AsyncStorage rather than localStorage,
// but the STORAGE_KEY + JSON shape are kept identical to the desktop for
// parity. 2b only manages task lists; calendars/contact lists join the shape
// when those surfaces land.

/** Persisted-selection storage key — identical to the desktop's. */
export const STORAGE_KEY = 'aperio.selection.v1';

export interface PersistedSelection {
  taskLists?: string[];
  /** Mirror set: every task-list id the reconciler has ever seen for this
   *  user. Lets it tell "user actively unticked this" (in known, not in
   *  selection) apart from "freshly appeared, never seen" (not in known) — the
   *  latter is auto-selected. Absent on blobs minted before this field landed;
   *  the reconciler migrates that in one shot. */
  knownTaskListIds?: string[];
}

/** Per-container-type slice tracked together so the reconciler can atomically
 *  update both the user's selection and the set of ids it has seen before.
 *  `known === null` means "never reconciled yet" — first run, or a persisted
 *  blob from before this field existed. */
export interface SelectionSlice {
  selected: Set<string>;
  known: Set<string> | null;
}

/**
 * Selection reconciliation against the latest list — runs every time the
 * backing list refreshes. Three distinct cases:
 *
 *   1. **First-ever run** (`prev.selected` empty AND `prev.known` null):
 *      default to selecting every item that passes `autoSelectNew` (or all
 *      items if no filter given).
 *
 *   2. **Existing user upgrade** (`prev.selected` populated AND `prev.known`
 *      null): persisted blob from before known-tracking existed. Freeze
 *      `known := selected ∪ list-ids` so we don't surprise-select lists the
 *      user had silently unticked under the old reconciler. Their current
 *      selection stays exactly as it was.
 *
 *   3. **Steady state** (`prev.known` non-null): keep selected ids that still
 *      exist; auto-select any id we have NEVER seen before (passing
 *      `autoSelectNew`); leave already-known-but-unselected ids alone — those
 *      were explicitly unticked.
 *
 * Returned `known` always covers exactly the current list (dropping ids that
 * disappeared, adding freshly-arrived ones) so the next reconcile has a clean
 * baseline.
 */
export function reconcileSelectionTracked<T extends { id: string }>(
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

  // Case 2: upgrade from pre-known-tracking storage. Freeze known to
  // "everything we know about right now" — both currently-selected and
  // currently-visible. Subsequent reconciles treat anything else as new.
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

/** Add `id` if absent, remove it if present — returning a NEW set. */
export function toggleSet(prev: Set<string>, id: string): Set<string> {
  const next = new Set(prev);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  return next;
}
