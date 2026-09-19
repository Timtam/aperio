import { calendarCurrentUserEmail } from '../api/client';

/**
 * Cache for "who am I on this calendar?" — the connected account's email.
 *
 * `calendarCurrentUserEmail` is a LIVE provider call (Graph `GET /me`, Google
 * `GET /calendars/primary`), not a local read, so we cache the answer per
 * calendar. EventRsvp is its only user: it finds the account's own attendee
 * row to answer an invitation. Whether the account organizes an event is not
 * decided here but by the adapter on read (`organized_elsewhere`, decision
 * 70a), which the delete paths and the chip menu read through the shared
 * `cancellationNotice`.
 *
 * The identity is effectively immutable for the life of an account, so there's
 * no invalidation; the cache is dropped when the page reloads.
 */
const cache = new Map<string, string | null>();
const inflight = new Map<string, Promise<string | null>>();

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
