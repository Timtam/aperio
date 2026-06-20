// A tiny local-mutation bus for calendar data. The task side bumps the task
// store's `dataVersion` after a write (which app-level consumers watch); calendar
// writes have no such store, so this lets app-level consumers — the app-icon
// badge — refresh after a LOCAL event create / edit / delete / exdate. (External
// background-cache refreshes arrive separately via cacheObserver's 'calendar'
// channel.) A leaf module with no imports, so the api layer can call it without
// a cycle.

const listeners = new Set<() => void>();

/** Fire after a local calendar write so subscribers re-read. */
export function notifyCalendarChanged(): void {
  listeners.forEach((l) => l());
}

/** Subscribe to local calendar writes; returns an unsubscribe. */
export function subscribeCalendarChanged(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
