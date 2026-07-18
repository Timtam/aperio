/**
 * Selection reconciliation for the sidebar's container sets (calendars /
 * task lists / contact lists) — extracted from CalendarStore so the
 * cold-listing semantics below are directly unit-testable.
 */

/** Per-container-type slice tracked together so the reconciler
 *  can atomically update the user's selection, the set of ids it's
 *  seen before, and each id's owning account. `known === null`
 *  means "never reconciled yet" — either first run or a
 *  localStorage blob from before this field existed. */
export interface SelectionSlice {
  selected: Set<string>;
  known: Set<string> | null;
  /** id → account_id, learned from every listing the id appeared in. */
  origin: Record<string, string>;
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
 *      unticked at some point.
 *
 * ## Cold listings are not removals
 *
 * At startup a listing is routinely PARTIAL: each account's slice is
 * served from its snapshot, and a cold or transiently-failing account
 * contributes NOTHING for a while. Dropping its ids here would (a)
 * churn the selection — every data hook keyed on the selected-id set
 * restarts cold and repaints progressively (the app-start day-count
 * oscillation), and (b) forget the ids ever existed, so previously
 * UNTICKED calendars came back auto-selected when the listing warmed.
 *
 * The reconciler therefore treats an id as genuinely removed ONLY
 * when its owning account (learned into `origin` from every listing
 * the id appeared in) answered this listing WITH content and no
 * longer includes it. An id whose account is absent from the listing
 * — cold snapshot, failed refresh, or a deleted account — is retained
 * in `selected`/`known` untouched; its reads simply return empty
 * until the account warms (or forever, harmlessly, if the account is
 * gone; the sidebar only renders ids present in the listing either
 * way). Ids with no recorded origin (pre-upgrade blobs) are retained
 * conservatively too.
 *
 * `autoSelectNew` lets the caller veto the auto-select default per
 * item — used by the contact-list reconciler to keep heavy
 * read-only lists (the EWS GAL) opt-in.
 */
export function reconcileSelectionTracked<
  T extends { id: string; account_id: string },
>(
  prev: SelectionSlice,
  list: T[],
  autoSelectNew?: (item: T) => boolean,
): SelectionSlice {
  const listIds = new Set(list.map((x) => x.id));
  const isNewDefaultOn = (item: T) =>
    autoSelectNew ? autoSelectNew(item) : true;
  const originFrom = (base: Iterable<string>): Record<string, string> => {
    const origin: Record<string, string> = {};
    for (const id of base) {
      if (prev.origin[id] !== undefined) origin[id] = prev.origin[id];
    }
    for (const item of list) origin[item.id] = item.account_id;
    return origin;
  };

  // Case 1: first-ever run.
  if (prev.selected.size === 0 && prev.known === null) {
    return {
      selected: new Set(list.filter(isNewDefaultOn).map((x) => x.id)),
      known: new Set(listIds),
      origin: originFrom([]),
    };
  }

  // Case 2: upgrade from pre-known-tracking localStorage. Freeze
  // known to "everything we know about right now" — both the
  // currently-selected and the currently-visible. Subsequent
  // reconciles will treat anything else as truly new. The selection
  // is kept verbatim (missing ids may belong to a still-cold
  // account; the steady-state rule sorts them out once origins are
  // learned).
  if (prev.known === null) {
    const known = new Set<string>(prev.selected);
    listIds.forEach((id) => known.add(id));
    return {
      selected: new Set(prev.selected),
      known,
      origin: originFrom(known),
    };
  }

  // Case 3: steady state. Accounts that answered with content — only
  // their ids can be genuinely removed.
  const accountsWithContent = new Set(list.map((x) => x.account_id));
  const genuinelyRemoved = (id: string): boolean => {
    if (listIds.has(id)) return false;
    const origin = prev.origin[id];
    return origin !== undefined && accountsWithContent.has(origin);
  };

  const selected = new Set<string>();
  prev.selected.forEach((id) => {
    if (!genuinelyRemoved(id)) selected.add(id);
  });
  for (const item of list) {
    if (!prev.known.has(item.id) && isNewDefaultOn(item)) {
      selected.add(item.id);
    }
  }
  const known = new Set<string>();
  prev.known.forEach((id) => {
    if (!genuinelyRemoved(id)) known.add(id);
  });
  listIds.forEach((id) => known.add(id));
  return { selected, known, origin: originFrom(known) };
}
