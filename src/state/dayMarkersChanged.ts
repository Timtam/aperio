import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';

/**
 * One signal for "the day markers moved", from either direction.
 *
 * Two things change this data and both have to reach the same readers:
 *
 *  - a LOCAL write — renaming a marker in settings has to reach the day view's
 *    summary, which is a different component tree holding its own copy;
 *  - a SYNC ROUND — a day ticked on the phone has to reach the desktop, which
 *    is the whole point of the markers being synced at all. Without it the row
 *    sat in SQLite while the running app showed an unmarked day until the next
 *    launch.
 *
 * Deliberately payload-free. The vocabulary is a handful of rows and a view's
 * summaries come from one range query, so every listener re-reads all of its
 * own data regardless; naming what moved would buy nothing and would have to
 * be kept honest at four write sites.
 */

type Listener = () => void;

const listeners = new Set<Listener>();

/** Tell every reader to re-read. Call after a local write lands. */
export function notifyDayMarkersChanged(): void {
  if (suppressDepth > 0) {
    suppressedOne = true;
    return;
  }
  listeners.forEach((l) => l());
}

// A burst of writes that is one logical change — a reorder rewrites every row
// whose position shifted. Without this the listeners would re-read between the
// rows and paint a half-applied order, which for a screen reader means the list
// moving under the cursor mid-action.
//
// Notifications raised inside the burst collapse into exactly one at the end,
// and only if something actually fired — a burst that wrote nothing stays
// silent.
let suppressDepth = 0;
let suppressedOne = false;

export async function duringDayMarkerBurst<T>(op: () => Promise<T>): Promise<T> {
  suppressDepth += 1;
  try {
    return await op();
  } finally {
    suppressDepth -= 1;
    if (suppressDepth === 0 && suppressedOne) {
      suppressedOne = false;
      listeners.forEach((l) => l());
    }
  }
}

/** Subscribe; returns an unsubscribe fn. */
export function subscribeDayMarkersChanged(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// The backend half, subscribed once for the whole app rather than per hook:
// several views can be mounted at once and they would otherwise each hold
// their own Tauri listener for the same event.
let backendWired = false;

function wireBackendOnce(): void {
  if (backendWired) return;
  backendWired = true;
  void listen('day-markers-changed', () => {
    notifyDayMarkersChanged();
  }).catch(() => {
    // No Tauri host (tests, a browser preview). Local writes still notify;
    // only the cross-device half is missing, and there are no other devices.
    backendWired = false;
  });
}

/**
 * Re-read whenever the day markers move, locally or from another device.
 *
 * The handler is read through a ref so callers need no `useCallback` and the
 * subscription survives their re-renders.
 */
export function useDayMarkersChanged(onChanged: () => void): void {
  const handlerRef = useRef(onChanged);
  handlerRef.current = onChanged;

  useEffect(() => {
    wireBackendOnce();
    return subscribeDayMarkersChanged(() => handlerRef.current());
  }, []);
}
