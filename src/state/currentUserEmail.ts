import { calendarCurrentUserEmail } from '../api/client';

/**
 * Cache for "who am I on this calendar?" — the connected account's email.
 *
 * `calendarCurrentUserEmail` is a LIVE provider call (Graph `GET /me`, Google
 * `GET /calendars/primary`), not a local read, so calling it on an interaction
 * path (the chip context menu, which must open instantly) would stall the UI —
 * and hang while offline. We cache the answer per calendar so the first opener
 * of a meeting (EventRsvp, the editor) warms it and later callers read it
 * synchronously with {@link peekCalendarUserEmail}.
 *
 * The identity is effectively immutable for the life of an account, so there's
 * no invalidation; the cache is dropped when the page reloads.
 */
const cache = new Map<string, string | null>();
const inflight = new Map<string, Promise<string | null>>();

/** Synchronous peek. `undefined` until the email has been resolved at least
 *  once for this calendar; `string | null` once known. */
export function peekCalendarUserEmail(
  calendarId: string,
): string | null | undefined {
  return cache.has(calendarId) ? cache.get(calendarId) : undefined;
}

/** Resolve (and cache) the connected account's email for `calendarId`.
 *  Concurrent callers share one in-flight request. */
export function resolveCalendarUserEmail(
  calendarId: string,
): Promise<string | null> {
  if (cache.has(calendarId)) {
    return Promise.resolve(cache.get(calendarId) ?? null);
  }
  const existing = inflight.get(calendarId);
  if (existing) return existing;
  const p = calendarCurrentUserEmail(calendarId)
    .then((email) => {
      cache.set(calendarId, email);
      inflight.delete(calendarId);
      return email;
    })
    .catch((err) => {
      inflight.delete(calendarId);
      throw err;
    });
  inflight.set(calendarId, p);
  return p;
}

/** Kick off a resolve to warm the cache; ignore the result/errors. Used to
 *  prime the cache off the interaction path (e.g. the first right-click on a
 *  meeting, so the cancel/notify choice is available on the next one). */
export function warmCalendarUserEmail(calendarId: string): void {
  void resolveCalendarUserEmail(calendarId).catch(() => {});
}
