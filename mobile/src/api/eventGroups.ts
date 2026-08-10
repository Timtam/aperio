// Which events mean the same appointment — the mobile twin of the desktop
// `group_events` / `ungroup_event` / `event_groups_for_events` commands.
//
// Nothing here reaches a provider: grouping two events changes neither of
// them, and ungrouping leaves both exactly as they were.

import type { EventGroup, SuggestionDecline } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';

/** A member on the way in: the reference plus its signature at this moment. */
export interface NewGroupMember {
  calendar_id: string;
  /** Series master id. */
  event_id: string;
  title: string;
  starts_at: string;
}

/** Declare that these events mean the same appointment.
 *
 *  Joins an existing group when exactly one of them is already in one — the
 *  natural "and this one too". Rejects with a conflict when two of them are in
 *  DIFFERENT groups, because merging two claims about what an appointment is
 *  cannot be inferred from a request that did not ask for it. */
export const groupEvents = async (
  members: NewGroupMember[],
): Promise<EventGroup> =>
  JSON.parse(await CalFfi.groupEventsJson(JSON.stringify(members))) as EventGroup;

/** Take one event out of its group.
 *
 *  `null` when that dissolved the group (fewer than two members left) or the
 *  event was not grouped at all. */
export const ungroupEvent = async (
  calendarId: string,
  eventId: string,
): Promise<EventGroup | null> => {
  const json = await CalFfi.ungroupEventJson(calendarId, eventId);
  return json == null ? null : (JSON.parse(json) as EventGroup);
};

/** Dissolve a whole group. The events themselves are untouched. */
export const dissolveEventGroup = (groupId: string): Promise<void> =>
  CalFfi.dissolveEventGroup(groupId);

/** Every group any of these events belongs to — whole, including members
 *  outside the range asked about. */
export const eventGroupsForEvents = async (
  events: { calendar_id: string; event_id: string }[],
): Promise<EventGroup[]> =>
  JSON.parse(
    await CalFfi.eventGroupsForEventsJson(JSON.stringify(events)),
  ) as EventGroup[];

/** Point one member at the id its event carries now.
 *
 *  A repair of Aperio's own bookkeeping — the same events mean the same
 *  appointment before and after — so it is applied silently by whichever view
 *  noticed, never announced as a change the user made. */
export const healEventGroupMember = (payload: {
  group_id: string;
  calendar_id: string;
  old_event_id: string;
  new_event_id: string;
}): Promise<void> =>
  CalFfi.healEventGroupMember(
    payload.group_id,
    payload.calendar_id,
    payload.old_event_id,
    payload.new_event_id,
  );

/** Record that two events are NOT the same appointment.
 *
 *  Silences the OFFER only — grouping them by hand still works and never
 *  consults this. */
export const declineGroupSuggestion = (
  first: { calendar_id: string; event_id: string },
  second: { calendar_id: string; event_id: string },
): Promise<void> =>
  CalFfi.declineGroupSuggestionJson(JSON.stringify(first), JSON.stringify(second));

/** Every pair the user has said is not one appointment. */
export const groupSuggestionDeclines = async (): Promise<SuggestionDecline[]> =>
  JSON.parse(await CalFfi.groupSuggestionDeclinesJson()) as SuggestionDecline[];
