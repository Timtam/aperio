// Which events mean the same appointment (DESIGN-event-groups.md).
//
// The same commitment routinely exists several times over: once in the work
// calendar so colleagues see it, once copied into a private calendar because
// that is the one a voice assistant reads out, and again in each colleague's
// calendar Aperio also reads. To every provider those are unrelated events. A
// group is Aperio's statement that they are not.
//
// Shared, because both frontends have to index the same answer the same way —
// a group read differently on the phone than on the desktop is worse than no
// group at all.

// The two shapes are DECLARED IN RUST (`cal_core::event_group`) and generated
// from there. They were written out here as well, in step with by hand and by a
// comment saying so — the same arrangement that let the container rows drift
// apart and offer a repeat shape Vikunja could not store.
//
// What stays below is not the domain: it is the JavaScript around it. A `Map`
// is a JavaScript structure and a `Map` key is a JavaScript string, so the
// indexing helpers belong on this side for the same reason `eventKey.ts` does
// — moving them would ship React's and V8's requirements into `cal-core`.
export type { EventGroup } from './generated/EventGroup';
export type { EventGroupMember } from './generated/EventGroupMember';

import type { EventGroup } from './generated/EventGroup';

/**
 * The key both frontends index members by.
 *
 * JSON rather than a joined string: a provider id may contain any character,
 * including whatever separator looked safe, and `("a b", "c")` must not
 * collide with `("a", "b c")` — a collision here would claim a stranger
 * belongs to a group. JSON escapes the ambiguity away and stays readable in a
 * debugger, which a control character does not (this line used to hold a
 * literal NUL, which also made the whole file binary to git).
 */
export function eventGroupMemberKey(
  calendarId: string,
  eventId: string,
): string {
  return JSON.stringify([calendarId, eventId]);
}

/**
 * Index groups by member, so a rendered row can ask "am I in one?" in O(1).
 *
 * Takes the groups WHOLE — as the backend returns them — because a group only
 * reads as a whole ("this and three others"), and the members outside the
 * rendered range are exactly the ones that make the count honest.
 */
export function indexEventGroups(
  groups: readonly EventGroup[],
): Map<string, EventGroup> {
  const byMember = new Map<string, EventGroup>();
  for (const group of groups) {
    for (const member of group.members) {
      byMember.set(eventGroupMemberKey(member.calendar_id, member.event_id), group);
    }
  }
  return byMember;
}

/**
 * The member payload for a grouping call: the reference plus the signature.
 *
 * Built from the event as it is RIGHT NOW, on purpose — the signature's job is
 * to re-find the event later, and the most recent title and start are the best
 * chance of that.
 */
export function memberFromEvent(event: {
  id: string;
  calendar_id: string;
  title?: string | null;
  start?: string | null;
}): {
  calendar_id: string;
  event_id: string;
  title: string;
  starts_at: string;
} {
  return {
    calendar_id: event.calendar_id,
    event_id: event.id,
    title: event.title ?? '',
    starts_at: event.start ?? '',
  };
}
