// One signal for "the day markers moved", from either direction. Twin of the
// desktop src/state/dayMarkersChanged.ts — same contract, same reasoning.
//
// Two things change this data and both have to reach the same readers:
//
//  - a LOCAL write — renaming a marker in settings has to reach the day list's
//    summary, which is a different screen holding its own copy;
//  - a SYNC ROUND — a day ticked on the desktop has to reach the phone, which
//    is the whole point of the markers being synced at all.
//
// Deliberately payload-free: the vocabulary is a handful of rows and a screen's
// summaries come from one range query, so every listener re-reads all of its
// own data regardless.
//
// Not folded into `notifyDataReload`: that one nudges the calendar/tasks/
// contacts categories through a coalescer sized for warm-pass bursts, and day
// markers are neither of those categories nor bursty. Riding it would mean
// every external cache update re-read the vocabulary for nothing.

import { useEffect, useRef } from 'react';

type Listener = () => void;

const listeners = new Set<Listener>();

/** Tell every reader to re-read. Call after a local write lands, or after a
 *  sync round reports it carried day-marker data. */
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

/**
 * Re-read whenever the day markers move, locally or from another device.
 *
 * The handler is read through a ref so callers need no `useCallback` and the
 * subscription survives their re-renders.
 */
export function useDayMarkersChanged(onChanged: () => void): void {
  const handlerRef = useRef(onChanged);
  handlerRef.current = onChanged;

  useEffect(() => subscribeDayMarkersChanged(() => handlerRef.current()), []);
}
