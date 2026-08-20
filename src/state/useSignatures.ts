import { useCallback, useEffect, useState } from 'react';

import type { Signature } from '@aperio/shared';

import { getUserPref, setUserPref } from '../api/client';

/**
 * The user's signature blocks, and which one a calendar reaches for.
 *
 * Stored in the SYNCED `user_prefs` rather than in a table of their own —
 * `calendar.<id>.defaultReminders` next door already keeps a JSON list there,
 * and this is the same shape of data: a handful of rows the user edits rarely
 * and reads often. A table would have bought per-row merge across devices at
 * the cost of a migration, three sync events, a snapshot field and fourteen FFI
 * methods, for a list most people will change twice a year.
 *
 * The cost is honest and worth naming: two devices editing DIFFERENT signatures
 * between two sync rounds keep the later write whole, not a merge of both.
 */

const LIST_KEY = 'signatures.list';
/** Which signature a calendar offers. Per calendar, like default reminders. */
const bindingKey = (calendarId: string) => `calendar.${calendarId}.signature`;

/** Tolerant of anything: a corrupt blob means "no signatures", never a crash
 *  and never a half-parsed row that would be written back over good data. */
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
    return [];
  }
}

export interface SignatureStore {
  signatures: Signature[];
  loading: boolean;
  /** Write the whole list back. */
  save: (next: Signature[]) => Promise<void>;
  /** The signature bound to `calendarId`, or null when it has none. */
  forCalendar: (calendarId: string) => Signature | null;
  /** Bind (or, with null, unbind) a calendar's signature. */
  bind: (calendarId: string, signatureId: string | null) => Promise<void>;
}

export function useSignatures(calendarIds: readonly string[] = []): SignatureStore {
  const [signatures, setSignatures] = useState<Signature[]>([]);
  const [bindings, setBindings] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  // The ids as a stable string, so the effect re-runs when the SET changes
  // rather than on every render that rebuilds the array.
  const idsKey = [...calendarIds].sort().join(' ');

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const raw = await getUserPref(LIST_KEY);
        if (!cancelled) setSignatures(parse(raw));
        const ids = idsKey === '' ? [] : idsKey.split(' ');
        const pairs = await Promise.all(
          ids.map(async (id) => [id, await getUserPref(bindingKey(id))] as const),
        );
        if (cancelled) return;
        setBindings(
          Object.fromEntries(
            pairs.filter((p): p is [string, string] => !!p[1]),
          ),
        );
      } catch {
        // Backend unreachable — an empty list reads as "none yet", and the
        // next open re-reads. Nothing is written back from a failed read.
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [idsKey]);

  const save = useCallback(async (next: Signature[]) => {
    setSignatures(next);
    await setUserPref(LIST_KEY, JSON.stringify(next));
  }, []);

  const bind = useCallback(
    async (calendarId: string, signatureId: string | null) => {
      setBindings((prev) => {
        const copy = { ...prev };
        if (signatureId) copy[calendarId] = signatureId;
        else delete copy[calendarId];
        return copy;
      });
      // An empty string, not a deletion: the same marker the default-reminder
      // editor writes, so "deliberately none" survives as a stored answer.
      await setUserPref(bindingKey(calendarId), signatureId ?? '');
    },
    [],
  );

  const forCalendar = useCallback(
    (calendarId: string) => {
      const id = bindings[calendarId];
      return signatures.find((s) => s.id === id) ?? null;
    },
    [bindings, signatures],
  );

  return { signatures, loading, save, forCalendar, bind };
}
