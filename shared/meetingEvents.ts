/**
 * Hiding a provider-side meeting that already has a calendar entry.
 *
 * A videoconference account contributes a read-only calendar of its own
 * meetings, so a meeting created in the provider's web UI — which has no
 * calendar entry anywhere — still shows up. But most meetings DO have one:
 * Aperio's own event, or the invitation Outlook wrote. Left alone, those appear
 * twice.
 *
 * The filter is the **join URL**, and that choice is the whole point. It is
 * what the provider issued, what the event carries, and what identifies the
 * meeting to everyone involved — an exact key, not a resemblance. Matching on
 * title and time instead would be worse than it looks: Aperio writes the
 * event's own title into the meeting it creates, so title equality carries
 * almost no evidence, and the times drift apart precisely when an event is
 * moved, which is when a user most wants the two seen as one.
 *
 * This runs in the view layer because that is the only place where all of a
 * window's events are in hand. An adapter is asked for one calendar's events
 * and cannot know what the others hold.
 */

/**
 * Whether an event came from a videoconference account's meetings calendar.
 *
 * A TWIN of `cal_core::is_meeting_calendar`, and knowingly so. Crossing the
 * door for a suffix test would cost a JSON round trip per row inside a render,
 * which is the opposite of what moving the filter below just bought. It goes
 * when its last TypeScript caller does — `meetingLinkGrouping.ts`, which has
 * not crossed yet.
 *
 * The suffix itself is `cal_core::MEETINGS_CALENDAR_SUFFIX`, which is also
 * what `host_core::vc_calendar` mints. Three readers, one string.
 */
export function isMeetingCalendarEvent(event: {
  calendar_id?: string | null;
}): boolean {
  return (event.calendar_id ?? '').endsWith('::meetings');
}

/**
 * This surface's door into `cal_core::meeting_events`.
 *
 * The whole window crosses at once and the answer is POSITIONS, because the
 * caller is holding the rows already.
 */
export interface MeetingDuplicateFilter {
  /** A `[{calendar_id, location?, description?, grouped}]` array in, the
   *  positions that survive out. */
  withoutDuplicateMeetingsJson(eventsJson: string): string;
}

let installedFilter: MeetingDuplicateFilter | null = null;

/** Bind this surface's door into the core. */
export function installMeetingDuplicateFilter(filter: MeetingDuplicateFilter): void {
  installedFilter = filter;
}

/**
 * Drop meetings-calendar events whose meeting is already represented by a real
 * calendar event in the same set.
 *
 * Order-independent and stable: real events are never dropped, only the
 * synthesized ones, and only when something else in view already shows that
 * exact meeting.
 *
 * The rule is `cal_core::meeting_events::without_duplicate_meetings`. It used
 * to be written out here, and it read each row's link through the detection
 * door once on the way in and again on the way out — so a day view crossed
 * that boundary twice per row to answer one question about the set. It crosses
 * once now, with the same bytes.
 */
export function withoutDuplicateMeetings<
  T extends {
    calendar_id?: string | null;
    location?: string | null;
    description?: string | null;
  },
>(
  events: T[],
  /**
   * Whether this row is already a member of a group (Stufe 4).
   *
   * A grouped meeting row must NOT be dropped here: the folding is what hides
   * it then, and it hides it while COUNTING it — the row says "2×" and the
   * group can be opened. Dropped first, the count would be a lie about a row
   * that is not there.
   *
   * This is also the transition: as automatic grouping spreads, this filter
   * quietly stops applying to the pairs it covers, and what is left is the
   * case it was written for — a meeting whose partner is not in view.
   */
  isGrouped: (event: T) => boolean = () => false,
): T[] {
  if (installedFilter === null) {
    // Loud, not a local fallback — a fallback would be the second
    // implementation all over again, and its failure is silent: a meeting the
    // user really has, vanishing with nothing to say so.
    throw new Error(
      'withoutDuplicateMeetings used before installMeetingDuplicateFilter() — ' +
        'the surface must bind its door into cal_core::meeting_events at startup',
    );
  }
  const answer = installedFilter.withoutDuplicateMeetingsJson(
    JSON.stringify(
      events.map((event) => ({
        calendar_id: event.calendar_id ?? '',
        location: event.location ?? null,
        description: event.description ?? null,
        grouped: isGrouped(event),
      })),
    ),
  );
  const keep = JSON.parse(answer) as number[];
  // The SAME array back when nothing was dropped, which is the commonest case
  // by far — most days hold no meeting row at all. Callers put this behind a
  // `useMemo`, and a fresh array on every pass would make its identity useless
  // to anything downstream that keys on it. The rule this replaces had the
  // property in a narrower form (it returned early only when no link was
  // claimed); keeping it whenever the answer is "all of them" is strictly
  // friendlier and never says anything different.
  if (keep.length === events.length) return events;
  return keep.map((at) => events[at]);
}
