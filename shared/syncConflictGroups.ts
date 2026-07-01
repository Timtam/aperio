// Grouping for the sync-conflict dialog. The applier records ONE conflict row
// per differing FIELD, so a single task edited on two devices surfaces as
// several rows (status + scheduled_date + completed_at …). Resolving them one
// field at a time is slow and error-prone — you almost never want to keep 2 of
// 3 fields from different devices. This groups the flat conflict list by the
// owning row (kind + id) so the UI can show one card per task/event with a
// "resolve all of this item" action, and reads the row's NAME instead of a raw
// UUID. Pure + platform-agnostic (desktop dialog + mobile screen share it).

/** The subset of a conflict record this module needs — the platform's full
 *  `SyncConflict` type structurally satisfies it. */
export interface GroupableConflict {
  row_kind: string;
  row_id: string;
}

/** One owning row (a task/event/…) and all its field-level conflicts, in the
 *  order they first appeared in the source list. */
export interface ConflictGroup<T extends GroupableConflict> {
  /** Stable key for React lists + label lookup: `"{row_kind}:{row_id}"`. */
  key: string;
  rowKind: string;
  rowId: string;
  conflicts: T[];
}

/** `"{row_kind}:{row_id}"` — the identity a conflict is grouped + labelled by. */
export function conflictGroupKey(c: GroupableConflict): string {
  return `${c.row_kind}:${c.row_id}`;
}

/**
 * Group a flat conflict list by owning row (kind + id), preserving first-seen
 * order both for the groups and for the conflicts within each. A single-field
 * conflict is just a group of one — the UI renders every group the same way.
 */
export function groupSyncConflicts<T extends GroupableConflict>(
  conflicts: readonly T[],
): ConflictGroup<T>[] {
  const groups = new Map<string, ConflictGroup<T>>();
  for (const c of conflicts) {
    const key = conflictGroupKey(c);
    let group = groups.get(key);
    if (!group) {
      group = { key, rowKind: c.row_kind, rowId: c.row_id, conflicts: [] };
      groups.set(key, group);
    }
    group.conflicts.push(c);
  }
  return [...groups.values()];
}
