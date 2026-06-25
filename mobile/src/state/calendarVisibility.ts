import { useCallback, useEffect, useState } from 'react';
import AsyncStorage from '@react-native-async-storage/async-storage';

// Per-device calendar visibility: which calendars the user has HIDDEN from the
// calendar views (and the event-target pickers). Stored as a hidden-set (an
// empty set = everything visible) so a newly-appearing calendar is visible by
// default — no reconciler needed. Persisted in AsyncStorage and mirrored into a
// module cache + a listener set, so the views (which filter) and the Calendars
// screen (which toggles) stay in sync. Host-LOCAL, mirroring the desktop's
// localStorage-backed sidebar selection and the contact-book visibility here.

const KEY = 'aperio.calendars.hidden.v1';
let cached = new Set<string>();
let loaded = false;
const listeners = new Set<() => void>();

async function load(): Promise<void> {
  if (loaded) return;
  loaded = true;
  try {
    const raw = await AsyncStorage.getItem(KEY);
    if (raw != null) cached = new Set(JSON.parse(raw) as string[]);
  } catch {
    // Best-effort; the default (nothing hidden) stays.
  }
  listeners.forEach((l) => l());
}

async function persist(): Promise<void> {
  listeners.forEach((l) => l());
  try {
    await AsyncStorage.setItem(KEY, JSON.stringify([...cached]));
  } catch {
    // Best-effort.
  }
}

/**
 * Reactive calendar visibility. Returns the current HIDDEN set — a FRESH
 * reference on every change, so a memo keyed on it re-runs — plus a `toggle`.
 * Loads the stored set on first mount; updates propagate to every mounted caller
 * (the views filter by it, the Calendars screen toggles it).
 */
export function useCalendarVisibility(): {
  hidden: Set<string>;
  toggle: (id: string) => void;
} {
  const [hidden, setHidden] = useState<Set<string>>(() => new Set(cached));
  useEffect(() => {
    void load();
    const sync = () => setHidden(new Set(cached));
    listeners.add(sync);
    return () => {
      listeners.delete(sync);
    };
  }, []);
  const toggle = useCallback((id: string) => {
    if (cached.has(id)) cached.delete(id);
    else cached.add(id);
    void persist();
  }, []);
  return { hidden, toggle };
}
