import { useEffect, useMemo, useRef, useState } from 'react';

import {
  eventGroupMemberKey,
  findHealableMembers,
  findStaleSignatures,
  type EventGroup,
} from '@aperio/shared';

import {
  eventGroupsForEvents,
  healEventGroupMember,
  refreshEventGroupSignature,
} from '../api/client';
import type { CalendarEvent } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';
import { useDialogState } from './dialogStateContext';

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
 */
export function useEventGroups(
  events: readonly CalendarEvent[],
  /** The range `events` covers. Given, the hook also REPAIRS members whose
   *  provider id changed — see below. Omitted, it only reads. */
  range?: { start: Date; end: Date },
): EventGroup[] {
  const { dataVersion } = useDialogState();
  const [groups, setGroups] = useState<EventGroup[]>([]);

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
      return;
    }
    let cancelled = false;
    const refs = refsKey.split('\n').map((entry) => {
      const [calendar_id, event_id] = JSON.parse(entry) as [string, string];
      return { calendar_id, event_id };
    });
    eventGroupsForEvents(refs)
      .then((found) => {
        if (!cancelled) setGroups(found);
      })
      .catch(() => {
        // A failed lookup means no folding this round — which is exactly what
        // the app did before groups existed. Never an empty day.
        if (!cancelled) setGroups([]);
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
    if (range == null || groups.length === 0) return;
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
  }, [groups, events, range, refsKey]);

  return groups;
}
