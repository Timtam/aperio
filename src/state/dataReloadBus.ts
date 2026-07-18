/**
 * Tiny module-level channel between programmatic data-changed producers
 * (e.g. the sync scheduler's round-end in `useSync`) and the mounted
 * `CacheSyncListener`, which owns the coalescer/throttle that turns
 * change signals into view refetches. Lives in its own file so the
 * component module keeps react-refresh compatibility.
 */

type PushScopes = (scopes: readonly string[]) => void;

let pushScopes: PushScopes | null = null;

/** Called by CacheSyncListener on mount/unmount to (de)register its
 *  coalescer entry point. */
export function registerDataReloadSink(sink: PushScopes | null): void {
  pushScopes = sink;
}

/**
 * Ask the data layer to refetch as if the given item scopes had emitted
 * `cache-updated` — routed through the SAME coalescer/throttle as real
 * backend emissions, so programmatic invalidations can't bypass the
 * reload-wave gating. Defaults to all item scopes. Dropped when no
 * listener is mounted yet (nothing rendered that could go stale).
 */
export function nudgeDataReload(
  scopes: readonly string[] = ['events', 'tasks', 'contacts'],
): void {
  pushScopes?.(scopes);
}
