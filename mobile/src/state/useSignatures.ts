import { useEffect, useState } from 'react';

import type { Signature } from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/prefs';
import { scheduleBackgroundPush } from '../api/syncTriggers';
import { subscribeCacheReload } from './cacheObserver';

/**
 * The user's signature blocks — the mobile twin of the desktop hook, on the
 * SAME synced pref keys, so a signature written on one device is the same
 * signature on the other.
 *
 * One module-level store with a listener fan-out, like the other mobile pref
 * hooks: the settings screen and an open editor stay in step, and hydration
 * never writes back. The store re-reads on every data reload (a sync round
 * that applied a peer's list), so an editor opened afterwards sees the
 * arrived list — it used to hydrate once and hold the pre-round value until
 * the next launch, which is exactly where "my desktop signatures aren't on
 * the phone" was observed.
 */

const LIST_KEY = 'signatures.list';
const bindingKey = (calendarId: string) => `calendar.${calendarId}.signature`;

function parse(raw: string | null): Signature[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (s): s is Signature =>
        typeof s === 'object' &&
        s !== null &&
        typeof (s as Signature).id === 'string' &&
        typeof (s as Signature).name === 'string' &&
        typeof (s as Signature).body === 'string',
    );
  } catch {
    // A corrupt blob means "no signatures" — never a crash, and never a
    // half-parsed row that would be written back over good data.
    return [];
  }
}

let cache: Signature[] = [];
let loaded = false;
let loading: Promise<void> | null = null;
/** Bumped by every local write. A read that started BEFORE a write must not
 *  land after it: the Host answers reads and writes on one serial queue, so a
 *  refresh dispatched just before a tap resolves with the pre-write list and
 *  would put it back over the optimistic one — the row the user just heard
 *  announced as added would vanish, and the next edit would write the stale
 *  list back. */
let writeGeneration = 0;
const listeners = new Set<() => void>();

function notify(): void {
  listeners.forEach((l) => l());
}

async function hydrate(): Promise<void> {
  if (loaded) return;
  if (loading) return loading;
  loading = (async () => {
    const generation = writeGeneration;
    try {
      const fresh = parse(await getUserPref(LIST_KEY));
      if (generation === writeGeneration) cache = fresh;
    } catch {
      // Host unreachable during init — an empty list reads as "none yet".
    } finally {
      loaded = true;
      loading = null;
      notify();
    }
  })();
  return loading;
}

/**
 * Re-read the list from the Host and tell every listener. Screens call this
 * on focus; the store itself calls it on every data reload (below). A write
 * that happened while the read was in flight wins — see `writeGeneration`.
 */
export async function refreshSignatures(): Promise<void> {
  const generation = writeGeneration;
  try {
    const fresh = parse(await getUserPref(LIST_KEY));
    if (generation !== writeGeneration) return;
    cache = fresh;
    loaded = true;
    notify();
  } catch {
    // Host unreachable — keep what we have; the next refresh tries again.
  }
}

// A sync round that applied a peer's data reloads every category; the list
// rides the calendar one (it is bound per calendar). Module-level and never
// unsubscribed: the store lives as long as the app does.
subscribeCacheReload('calendar', () => void refreshSignatures());

/** Write the whole list back and tell every listener. */
export async function saveSignatures(next: Signature[]): Promise<void> {
  writeGeneration += 1;
  cache = next;
  loaded = true;
  notify();
  await setUserPref(LIST_KEY, JSON.stringify(next));
  // A synced setting: push it now rather than at the next periodic round, the
  // way every other mobile mutation does.
  scheduleBackgroundPush();
}

export function useSignatures(): { signatures: Signature[]; loading: boolean } {
  const [signatures, setSignatures] = useState<Signature[]>(cache);
  const [isLoading, setIsLoading] = useState(!loaded);
  useEffect(() => {
    const listener = () => {
      setSignatures(cache);
      setIsLoading(false);
    };
    listeners.add(listener);
    void hydrate().then(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);
  return { signatures, loading: isLoading };
}

/** The signature bound to a calendar, or null. Read on demand rather than
 *  cached: an editor asks once, for one calendar. */
export async function signatureForCalendar(
  calendarId: string,
  signatures: readonly Signature[],
): Promise<Signature | null> {
  if (!calendarId) return null;
  try {
    const id = await getUserPref(bindingKey(calendarId));
    return signatures.find((s) => s.id === id) ?? null;
  } catch {
    return null;
  }
}

/** Bind (or, with null, unbind) a calendar's signature. */
export async function bindSignature(
  calendarId: string,
  signatureId: string | null,
): Promise<void> {
  // An empty string, not a deletion: "deliberately none" has to survive as a
  // stored answer, exactly as the default-reminder editor writes it.
  await setUserPref(bindingKey(calendarId), signatureId ?? '');
  scheduleBackgroundPush();
}
