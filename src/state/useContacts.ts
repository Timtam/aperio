import { useEffect, useMemo, useState } from 'react';

import { getContacts } from '../api/client';
import type { Contact } from '../api/types';
import { useCalendarStore } from './calendarStoreContext';
import { useDialogState } from './dialogStateContext';

/**
 * Pull contacts from every selected list and return the aggregated,
 * alphabetically-sorted list.
 *
 * Stale-while-revalidate cache: hit by the selected-list-id set + the
 * dialog `dataVersion` counter, exactly like `useTasks`. The cache
 * wipes itself on any data-version bump so a create / update / delete
 * is visible on the next render.
 *
 * The list view is responsible for handling large result sets (the
 * EWS GAL can carry ~2000 entries). With `aria-setsize` and
 * `content-visibility: auto` removed from the rendered options,
 * Chromium + NVDA handle a 2000-row listbox without freezing — the
 * earlier crash chain was driven by those two ARIA-related hooks,
 * not by raw row volume.
 */

type CacheKey = string;

const contactsCache = new Map<CacheKey, Contact[]>();
let cachedDataVersion = -1;

function ensureCacheVersion(version: number): void {
  if (version !== cachedDataVersion) {
    contactsCache.clear();
    cachedDataVersion = version;
  }
}

function cacheGet(key: CacheKey, version: number): Contact[] | undefined {
  ensureCacheVersion(version);
  return contactsCache.get(key);
}

function cacheSet(key: CacheKey, version: number, contacts: Contact[]): void {
  ensureCacheVersion(version);
  contactsCache.set(key, contacts);
}

/** Test-only escape hatch — wipes the cache between vitest runs. */
export function __resetContactsCacheForTests(): void {
  contactsCache.clear();
  cachedDataVersion = -1;
}

export function useContacts() {
  const { contactLists, selectedContactListIds, contactListsLoading } =
    useCalendarStore();
  const { dataVersion } = useDialogState();

  // The cache key folds in the *selected* subset of lists rather
  // than every known one. Ticking a previously-unticked list
  // produces a new key, which re-pulls only the relevant rows;
  // unticking shrinks the fan-out.
  const idsKey = useMemo(
    () =>
      contactLists
        .map((l) => l.id)
        .filter((id) => selectedContactListIds.has(id))
        .sort()
        .join(' '),
    [contactLists, selectedContactListIds],
  );

  const [contacts, setContacts] = useState<Contact[]>(
    () => cacheGet(idsKey, dataVersion) ?? [],
  );
  const [loading, setLoading] = useState<boolean>(
    () => cacheGet(idsKey, dataVersion) === undefined,
  );

  useEffect(() => {
    let cancelled = false;

    const cached = cacheGet(idsKey, dataVersion);
    if (cached) {
      setContacts(cached);
      setLoading(false);
    }

    // Wait for the CONTACT-LIST catalog before fetching (the gate the other
    // data hooks use, which this one was missing). Without it, the first
    // render — while the catalog is still loading and the selection is
    // therefore empty — would cache an empty result and flash "no contacts"
    // before the real fetch. Contacts only need their own catalog, so this
    // doesn't couple them to a slow calendar or task source.
    if (contactListsLoading) return;

    const ids = contactLists
      .map((l) => l.id)
      .filter((id) => selectedContactListIds.has(id));
    if (ids.length === 0) {
      setContacts([]);
      setLoading(false);
      cacheSet(idsKey, dataVersion, []);
      return;
    }

    if (!cached) setLoading(true);

    Promise.all(
      ids.map((id) =>
        getContacts(id).catch((err) => {
          // eslint-disable-next-line no-console
          console.warn('get_contacts failed for list', id, err);
          return [] as Contact[];
        }),
      ),
    ).then((batches) => {
      if (cancelled) return;
      const flat = batches.flat();
      flat.sort(contactOrder);
      const previous = cacheGet(idsKey, dataVersion);
      cacheSet(idsKey, dataVersion, flat);
      // Stale-while-revalidate: if the fresh fetch matches what we
      // already had (same length + same boundary ids after sort),
      // skip the state update. Dodges a needless re-render that
      // would re-create the contacts array reference and cascade
      // through every downstream useMemo / consumer effect.
      if (previous && shallowSameContacts(previous, flat)) {
        setLoading(false);
        return;
      }
      setContacts(flat);
      setLoading(false);
    });

    return () => {
      cancelled = true;
    };
    // contactLists / selectedContactListIds intentionally omitted —
    // `idsKey` is the stable projection, same trick the other data
    // hooks use.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [contactListsLoading, idsKey, dataVersion]);

  const contactListById = useMemo(() => {
    const map = new Map<string, (typeof contactLists)[number]>();
    contactLists.forEach((l) => map.set(l.id, l));
    return map;
  }, [contactLists]);

  return { contacts, loading, contactListById };
}

function contactOrder(a: Contact, b: Contact): number {
  // Case-insensitive sort on display_name. Numeric collation
  // doesn't help here — names rarely contain numbers, and when
  // they do the default lexical order is fine.
  return a.display_name.localeCompare(b.display_name, undefined, {
    sensitivity: 'base',
  });
}

/** Fast "same contacts after sort" check for the SWR path. We
 *  compare length + boundary ids + a few midpoints; if all of
 *  those agree, the lists are almost certainly identical, and a
 *  false positive only costs us up-to-the-next-mutation
 *  staleness (which the dataVersion bump on dialog close fixes
 *  anyway). Cheap: O(1) work regardless of list size. */
function shallowSameContacts(a: Contact[], b: Contact[]): boolean {
  if (a.length !== b.length) return false;
  if (a.length === 0) return true;
  if (a[0].id !== b[0].id) return false;
  if (a[a.length - 1].id !== b[b.length - 1].id) return false;
  if (a.length > 4) {
    const mid = a.length >> 1;
    if (a[mid].id !== b[mid].id) return false;
  }
  return true;
}
