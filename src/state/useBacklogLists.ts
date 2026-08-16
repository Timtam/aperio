import { useCallback, useEffect, useState } from 'react';

/**
 * Which task lists the BACKLOG hides, per device.
 *
 * Hiding a list in the sidebar takes it out of everything — the calendar days,
 * the pickers, the backlog. That is one switch doing two jobs, and the two
 * pull apart badly: a long household list is exactly the sort of thing that
 * swamps the backlog while its individual dated tasks are still wanted on the
 * days they fall on. Users were keeping the sidebar permanently open just to
 * flip lists in and out, which costs the width the backlog needed in the first
 * place.
 *
 * So this is a SECOND, narrower filter that only the backlog reads. It hides
 * rather than shows: a list the user has never heard of — created on another
 * device, or on the web — appears in the backlog by default, which is the
 * behaviour that cannot lose work. An "only these are shown" set would make a
 * new list silently invisible.
 *
 * Device-local in `localStorage`, deliberately NOT synced. The desktop with a
 * wide window and the laptop with a narrow one want different things here, and
 * it describes a screen rather than the data — the same reasoning as the theme
 * mode and the UI scale beside it.
 */

const STORAGE_KEY = 'aperio.backlog.hiddenLists';

/** Cross-component notification, so several rails (and the popup that edits
 *  it) stay in step without threading state through the store. */
type Listener = () => void;
const listeners = new Set<Listener>();

function read(): Set<string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    // Tolerate anything: this is a display filter, and a corrupt blob must
    // degrade to "show everything" rather than to an empty backlog.
    return Array.isArray(parsed)
      ? new Set(parsed.filter((x): x is string => typeof x === 'string'))
      : new Set();
  } catch {
    return new Set();
  }
}

function write(ids: Set<string>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...ids]));
  } catch {
    // Private mode / quota. The filter still holds for this session; losing it
    // at the next launch is not worth an error the user cannot act on.
  }
  listeners.forEach((l) => l());
}

export interface BacklogLists {
  /** Ids the backlog hides. Empty means "show all", including lists that did
   *  not exist when this was last written. */
  hidden: ReadonlySet<string>;
  /** Whether `listId` currently shows in the backlog. */
  shows: (listId: string) => boolean;
  /** Show or hide one list in the backlog. */
  setShown: (listId: string, shown: boolean) => void;
  /** Clear the filter — every list back in the backlog. */
  showAll: () => void;
}

export function useBacklogLists(): BacklogLists {
  const [hidden, setHidden] = useState<Set<string>>(read);

  useEffect(() => {
    const listener = () => setHidden(read());
    listeners.add(listener);
    // Another WINDOW of the same app (Tauri can have several) writing the key.
    // `storage` never fires in the window that wrote it, which is why the
    // in-process listener above exists too.
    const onStorage = (e: StorageEvent) => {
      if (e.key === null || e.key === STORAGE_KEY) listener();
    };
    window.addEventListener('storage', onStorage);
    return () => {
      listeners.delete(listener);
      window.removeEventListener('storage', onStorage);
    };
  }, []);

  const shows = useCallback((listId: string) => !hidden.has(listId), [hidden]);

  const setShown = useCallback((listId: string, shown: boolean) => {
    const next = read();
    if (shown) next.delete(listId);
    else next.add(listId);
    write(next);
  }, []);

  const showAll = useCallback(() => {
    write(new Set());
  }, []);

  return { hidden, shows, setShown, showAll };
}
