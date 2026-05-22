import { useEffect, useMemo, useState } from 'react';

import { getContacts } from '../api/client';
import type { Contact } from '../api/types';
import { useCalendarStore } from './CalendarStore';
import { useDialogState } from './DialogState';

/**
 * Pull contacts from every known contact list and return the
 * aggregated, alphabetically-sorted list.
 *
 * Phase 10a-3 keeps the surface simple: we always read every list
 * the store knows about. A future polish (10a-4) can add a
 * sidebar selection set and consume it the same way `useEvents` /
 * `useTasks` do, but contacts don't typically have the
 * many-books-but-only-show-one workflow that calendars do —
 * "show me everyone" is almost always the right default.
 *
 * Stale-while-revalidate cache: hit by the contact-list catalog
 * key and the dialog `dataVersion` counter, exactly like
 * `useTasks`. The cache wipes itself on any data-version bump so
 * a create / update / delete is visible on the next render.
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
  const { contactLists, loading: storeLoading } = useCalendarStore();
  const { dataVersion } = useDialogState();

  const idsKey = useMemo(
    () =>
      contactLists
        .map((l) => l.id)
        .sort()
        .join(' '),
    [contactLists],
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
    } else {
      setLoading(true);
    }

    if (storeLoading) return;

    const ids = contactLists.map((l) => l.id);
    if (ids.length === 0) {
      setContacts([]);
      setLoading(false);
      cacheSet(idsKey, dataVersion, []);
      return;
    }

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
      cacheSet(idsKey, dataVersion, flat);
      setContacts(flat);
      setLoading(false);
    });

    return () => {
      cancelled = true;
    };
    // contactLists intentionally omitted — `idsKey` is the stable
    // projection, same trick the other data hooks use.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storeLoading, idsKey, dataVersion]);

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
