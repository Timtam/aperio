// Finding a member again after the provider re-minted its id
// (DESIGN-event-groups.md, "Wie eine Gruppe überlebt").
//
// Event ids belong to the provider and change underneath us: a re-bootstrap
// remints them, moving an event between calendars remints it, and Exchange
// does it unprompted. A group that stored ids alone would lose limbs in
// silence — which the design calls the worst of the available failures,
// because a group that is quietly incomplete still looks authoritative.
//
// That is what the SIGNATURE is for. Membership records the title and start an
// event had when it joined, so a member whose id no longer resolves can be
// looked for rather than dropped.
//
// The rule is deliberately the same strict one detection uses: same calendar,
// same title, same start. A near match here would silently rewrite a group to
// point at the wrong appointment, which is worse than a group that is one
// member short and says so.

import type { EventGroup, EventGroupMember } from './eventGroups';
import { eventGroupMemberKey } from './eventGroups';

/** The minimum a row needs to stand in for a lost member. */
export interface HealableEvent {
  id: string;
  calendar_id: string;
  title: string;
  start: string;
  all_day?: boolean;
}

/** A member that was found again under a new id. */
export interface HealedMember {
  group_id: string;
  calendar_id: string;
  /** The id that no longer resolves. */
  old_event_id: string;
  /** The id the same appointment carries now. */
  new_event_id: string;
}

function normalizeTitle(title: string): string {
  return title.trim().toLowerCase().replace(/\s+/g, ' ');
}

function whenKey(value: string, allDay: boolean | undefined): string {
  return allDay ? value.slice(0, 10) : new Date(value).toISOString();
}

/**
 * Members whose id has gone stale and whose event is findable again.
 *
 * ## Why the range matters
 *
 * A member that is simply outside the rendered range is not missing — it is
 * elsewhere, and rewriting it because it is not on screen would be the bug
 * this prevents. So only members whose stored START falls inside the range are
 * considered: there, "no event with that id" really does mean the id changed,
 * because the event would otherwise be in hand.
 *
 * A member that cannot be found even so is left exactly as it is. It may be a
 * copy the user deleted, and dropping the membership on that suspicion would
 * quietly shrink a group nobody asked to change.
 */
export function findHealableMembers<E extends HealableEvent>(
  groups: readonly EventGroup[],
  eventsInRange: readonly E[],
  range: { start: Date; end: Date },
  seriesId: (event: E) => string,
): HealedMember[] {
  if (groups.length === 0) return [];
  const present = new Set(
    eventsInRange.map((ev) => eventGroupMemberKey(ev.calendar_id, seriesId(ev))),
  );
  const from = range.start.getTime();
  const to = range.end.getTime();

  const inRange = (member: EventGroupMember): boolean => {
    const at = new Date(member.starts_at).getTime();
    return Number.isFinite(at) && at >= from && at <= to;
  };

  const healed: HealedMember[] = [];
  for (const group of groups) {
    for (const member of group.members) {
      if (present.has(eventGroupMemberKey(member.calendar_id, member.event_id))) continue;
      if (!inRange(member)) continue;
      const wantedTitle = normalizeTitle(member.title);
      if (wantedTitle === '') continue;
      // Every candidate, not the first: a calendar can hold two events with
      // the same name at the same time, and picking one of them would send the
      // group to an appointment nobody pointed it at — silently, since the
      // repair says nothing. Ambiguity therefore heals NOTHING; the group
      // keeps a member it cannot resolve, which is visible and fixable, rather
      // than gaining one that is wrong and is not.
      const matches = eventsInRange.filter((ev) => {
        if (ev.calendar_id !== member.calendar_id) return false;
        if (normalizeTitle(ev.title) !== wantedTitle) return false;
        // Compared on the candidate's own footing: an all-day event agrees on
        // the DAY, a timed one on the instant.
        //
        // KNOWN LIMIT: a member's signature cannot say whether IT was all-day
        // — `Event.start` is an instant either way — so a timed member could in
        // principle match an all-day event on the same day. The uniqueness rule
        // above contains it: that only bites when the day holds exactly one
        // event with the same name, and it is all-day. Closing it properly
        // means widening the signature, which is a migration for a case nobody
        // has hit.
        return whenKey(ev.start, ev.all_day) === whenKey(member.starts_at, ev.all_day);
      });
      if (matches.length !== 1) continue;
      const replacement = matches[0];
      const newId = seriesId(replacement);
      // Already a member under the new id (both rows are in the group) —
      // nothing to rewrite, and rewriting would collide.
      if (present.has(eventGroupMemberKey(member.calendar_id, newId))) {
        if (group.members.some((m) => m.event_id === newId)) continue;
      }
      healed.push({
        group_id: group.id,
        calendar_id: member.calendar_id,
        old_event_id: member.event_id,
        new_event_id: newId,
      });
    }
  }
  return healed;
}
