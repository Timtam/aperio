import { useEffect, useState } from 'react';

import type { Signature } from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/prefs';

/**
 * The user's signature blocks — the mobile twin of the desktop hook, on the
 * SAME synced pref keys, so a signature written on one device is the same
 * signature on the other.
 *
 * One module-level store with a listener fan-out, like the other mobile pref
 * hooks: the settings screen and an open editor stay in step, and hydration
 * never writes back.
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
const listeners = new Set<() => void>();

async function hydrate(): Promise<void> {
  if (loaded) return;
  if (loading) return loading;
  loading = (async () => {
    try {
      cache = parse(await getUserPref(LIST_KEY));
    } catch {
      // Host unreachable during init — an empty list reads as "none yet".
    } finally {
      loaded = true;
      loading = null;
      listeners.forEach((l) => l());
    }
  })();
  return loading;
}

/** Write the whole list back and tell every listener. */
export async function saveSignatures(next: Signature[]): Promise<void> {
  cache = next;
  loaded = true;
  listeners.forEach((l) => l());
  await setUserPref(LIST_KEY, JSON.stringify(next));
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
}
