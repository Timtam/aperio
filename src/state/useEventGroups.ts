import { useEffect, useMemo, useRef, useState } from 'react';

import {
  eventGroupMemberKey,
  findHealableMembers,
  findMeetingLinkPairs,
  findStaleSignatures,
  indexEventGroups,
  memberFromEvent,
  withoutDuplicateMeetings,
  type EventGroup,
} from '@aperio/shared';

import {
  eventGroupsForEvents,
  groupEvents,
  groupSuggestionDeclines,
  healEventGroupMember,
  refreshEventGroupSignature,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';
import { useDialogState } from './dialogStateContext';

/** What a view renders, and what it knows about the groups behind it. */
export interface GroupedEvents {
  /** The groups any of the rows belong to, whole. */
  groups: EventGroup[];
  /**
   * The rows to show.
   *
   * The same events, minus the videoconference meetings whose appointment is
   * already in view and NOT grouped with them. A grouped one stays: folding is
   * what hides it then, and folding hides it while still counting it — and
   * stops hiding it the moment the two disagree about when the appointment is,
   * which is the case the plain filter can never see (the join URL still
   * matches, so it would go on hiding the meeting exactly when the mismatch
   * matters). See `DESIGN-event-groups.md`, Stufe 4.
   */
  events: CalendarEvent[];
}

/**
 * The groups behind the events a view is about to render
 * (`DESIGN-event-groups.md`).
 *
 * One query per rendered range, not one per row: a day with twenty events
 * should cost one call. The answer comes back as WHOLE groups, including the
 * members that fall outside the range — a group only reads as a whole ("this
 * and three others"), and the count has to stay honest even when a copy sits
 * in a calendar the user has switched off.
 *
 * Keyed on the series masters in view, so paging back to a day whose events
 * are already known does not re-ask. Re-runs on `dataVersion`, like everything
 * else that has to notice a mutation.
 *
 * Given a range it also does the three things that need a whole window in
 * hand: refreshing member signatures, healing members whose provider id
 * changed, and grouping a videoconference meeting with the appointment it
 * belongs to. All three are passes over evidence only a view has.
 */
export function useEventGroups(
  events: readonly CalendarEvent[],
  /** The range `events` covers. Given, the hook also REPAIRS members whose
   *  provider id changed — see below. Omitted, it only reads. */
  range?: { start: Date; end: Date },
): GroupedEvents {
  const { dataVersion } = useDialogState();
  const [groups, setGroups] = useState<EventGroup[]>([]);
  /**
   * The window `groups` actually describes.
   *
   * `groups` alone cannot say whether it is an answer or a starting value:
   * `[]` means both "not asked yet" and "there are none". And it is never
   * cleared when the window changes, so after paging it holds the PREVIOUS
   * day's answer. Anything that WRITES on the strength of it has to know the
   * difference — see the link pass below, which formed groups that already
   * existed because it read `[]` as "none".
   */
  const [groupsKey, setGroupsKey] = useState<string | null>(null);

  // The distinct (calendar, series) pairs as one stable string — the same
  // trick `useEvents` uses for its calendar set, and for the same reason: the
  // array identity changes on every render, its contents rarely do.
  //
  // The shared member key is JSON, so joining the entries with a newline
  // cannot collide with anything a provider might put in an id.
  const refsKey = useMemo(() => {
    const seen = new Set<string>();
    for (const ev of events) {
      seen.add(eventGroupMemberKey(ev.calendar_id, seriesIdOf(ev)));
    }
    return [...seen].sort().join('\n');
  }, [events]);

  useEffect(() => {
    if (refsKey === '') {
      setGroups([]);
      setGroupsKey('');
      return;
    }
    let cancelled = false;
    const refs = refsKey.split('\n').map((entry) => {
      const [calendar_id, event_id] = JSON.parse(entry) as [string, string];
      return { calendar_id, event_id };
    });
    eventGroupsForEvents(refs)
      .then((found) => {
        if (cancelled) return;
        setGroups(found);
        setGroupsKey(refsKey);
      })
      .catch(() => {
        // A failed lookup means no folding this round — which is exactly what
        // the app did before groups existed. Never an empty day.
        //
        // And the key is CLEARED, not left alone. An empty list from a failure
        // must not read as "there are no groups here" to anything that writes,
        // and leaving the key behind only achieved that while the failure was
        // the first fetch for this window: once it had succeeded once, a later
        // failure downgraded the groups to [] while the key still said they
        // described the window in hand — and the link pass would form groups
        // that already exist, which is the whole reason the key exists.
        if (!cancelled) {
          setGroups([]);
          setGroupsKey(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [refsKey, dataVersion]);

  /**
   * Point members at the ids their events carry now.
   *
   * Ids belong to the provider and change underneath us. A view that has a
   * range in hand can tell the difference between "that member is elsewhere"
   * and "that id resolves to nothing here", and the stored signature says
   * which event it was — so the repair happens where the evidence is.
   *
   * Silent, and deliberately so: the same events mean the same appointment
   * before and after, so there is nothing to tell the user. It stays on this
   * device too — every device has the same evidence and repairs itself, and
   * broadcasting a repair stamped "now" would outrank a dissolve someone else
   * had just made. The refreshed groups are read back rather than patched
   * locally, because the stored group is the answer.
   */
  // Every repair this hook has already attempted. Without it the effect feeds
  // itself: it heals, reads the groups back, `groups` changes, the effect runs
  // again — and a repair that CANNOT succeed (the write fails, or the arriving
  // group still names the old id) does that forever, one round trip per turn.
  const attempted = useRef(new Set<string>());
  useEffect(() => {
    if (range == null || groupsKey !== refsKey || groups.length === 0) return;
    // Keep the signatures describing the events as they ARE. Written once at
    // joining they went stale the first time the appointment moved — and then
    // the healing below, which searches by exactly them, could never match
    // again. Silent and local, like the heal.
    for (const stale of findStaleSignatures(groups, events, seriesIdOf)) {
      const key = `sig\n${stale.calendar_id}\n${stale.event_id}\n${stale.title}\n${stale.starts_at}`;
      if (attempted.current.has(key)) continue;
      attempted.current.add(key);
      void refreshEventGroupSignature(stale).catch(() => undefined);
    }
    const healable = findHealableMembers(groups, events, range, seriesIdOf).filter(
      (member) =>
        !attempted.current.has(
          `${member.group_id}\n${member.calendar_id}\n${member.old_event_id}\n${member.new_event_id}`,
        ),
    );
    if (healable.length === 0) return;
    for (const member of healable) {
      attempted.current.add(
        `${member.group_id}\n${member.calendar_id}\n${member.old_event_id}\n${member.new_event_id}`,
      );
    }
    let cancelled = false;
    void (async () => {
      for (const member of healable) {
        try {
          await healEventGroupMember(member);
        } catch {
          // A repair that fails is a repair not made; the group keeps the id
          // it had and the next render will try again.
        }
      }
      if (cancelled || refsKey === '') return;
      const refs = refsKey.split('\n').map((entry) => {
        const [calendar_id, event_id] = JSON.parse(entry) as [string, string];
        return { calendar_id, event_id };
      });
      const refreshed = await eventGroupsForEvents(refs).catch(() => null);
      if (!cancelled && refreshed != null) setGroups(refreshed);
    })();
    return () => {
      cancelled = true;
    };
  }, [groups, groupsKey, events, range, refsKey]);

  /**
   * Group a videoconference meeting with the appointment it belongs to.
   *
   * Unlike the two repairs above this WRITES a group, and the group syncs —
   * because it is a statement about what an appointment is, not bookkeeping.
   * Two devices doing it at once converge the way every other group does, by
   * timestamp.
   *
   * ## It may only act on evidence that describes THESE events
   *
   * `groupsKey !== refsKey` means the groups in hand answer a different
   * question — the fetch has not landed, or it landed for the window we have
   * since paged away from. Reading that as "these events are ungrouped" made
   * this pass re-form groups that already existed, on every cold load and
   * after every page step: a write, a sync-log entry and a fresh `updated_at`
   * per pair, for nothing. Worse, a re-affirmation stamped NOW outranks
   * another device's older dissolve — so the group the user pulled apart there
   * came back here.
   *
   * The refusals are read HERE rather than kept in a state of their own, and
   * that is the same rule again. As two effects they raced: after a sync round
   * carrying "group dissolved" plus its refusal, whichever promise resolved
   * first decided the render, and if it was the groups this pass ran with the
   * OLD refusals and re-created the group. Mobile always read them in one
   * chain; this now matches it.
   *
   * Guarded by the same `attempted` ref: a pair that cannot be grouped (the
   * two turn out to be in different groups, say) must be tried once and then
   * left alone, or every render would ask again.
   */
  useEffect(() => {
    if (range == null || groupsKey !== refsKey) return;
    // Cheap first look, with no refusals in hand: it can only ever find MORE
    // than the real answer, so nothing here means nothing to ask about — and
    // no reason to touch the database at all.
    if (findMeetingLinkPairs(events, groups, [], seriesIdOf).length === 0) return;
    let cancelled = false;
    void (async () => {
      const declines = await groupSuggestionDeclines().catch(() => null);
      // Not knowing what has been refused is not the same as nothing having
      // been refused.
      if (cancelled || declines == null) return;
      const pairs = findMeetingLinkPairs(events, groups, declines, seriesIdOf).filter(
        (pair) => {
          const key = `link\n${eventGroupMemberKey(pair.meeting.calendar_id, seriesIdOf(pair.meeting))}\n${eventGroupMemberKey(pair.event.calendar_id, seriesIdOf(pair.event))}`;
          if (attempted.current.has(key)) return false;
          attempted.current.add(key);
          return true;
        },
      );
      if (pairs.length === 0) return;
      let grouped = false;
      for (const pair of pairs) {
        try {
          await groupEvents([
            memberFromEvent({ ...pair.meeting, id: seriesIdOf(pair.meeting) }),
            memberFromEvent({ ...pair.event, id: seriesIdOf(pair.event) }),
          ]);
          grouped = true;
        } catch {
          // Refused (two different groups) or failed to write. Either way the
          // day looks exactly as it did before — the filter below still hides
          // the duplicate — and the pair is not asked about again.
        }
      }
      if (cancelled || !grouped || refsKey === '') return;
      const refs = refsKey.split('\n').map((entry) => {
        const [calendar_id, event_id] = JSON.parse(entry) as [string, string];
        return { calendar_id, event_id };
      });
      const refreshed = await eventGroupsForEvents(refs).catch(() => null);
      if (!cancelled && refreshed != null) setGroups(refreshed);
    })();
    return () => {
      cancelled = true;
    };
  }, [groups, groupsKey, events, range, refsKey]);

  const visible = useMemo(() => {
    const byMember = indexEventGroups(groups);
    return withoutDuplicateMeetings([...events], (event) =>
      byMember.has(eventGroupMemberKey(event.calendar_id, seriesIdOf(event))),
    );
  }, [events, groups]);

  return { groups, events: visible };
}
