import { calendarCurrentUserEmail } from '../api/calendar';

/**
 * Cache for "who am I on this calendar?" — the connected account's email.
 *
 * `calendarCurrentUserEmail` is a LIVE provider call (via the host FFI), not a
 * local read, so we cache the answer per calendar. EventRsvp is its only user:
 * it finds the account's own attendee row to answer an invitation. Whether the
 * account organizes an event is not decided here but by the adapter on read
 * (`organized_elsewhere`, decision 70a), which the delete flow
 * (`confirmDeleteEvent`) reads through the shared `cancellationNotice`. The
 * mobile twin of the desktop `currentUserEmail` cache. Identity is immutable
 * for an account's lifetime, so no invalidation — the cache dies with the JS
 * context.
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
