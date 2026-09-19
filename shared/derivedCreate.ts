/**
 * Who organizes the event a new one is made from (decision 72a).
 *
 * A create derived from an existing event (an occurrence carved out of its
 * series, the tail of a split, a copy, a carried or detached one) takes that
 * event's invitees along. The provider may still list the organizer among
 * them, so the create carries the source's organizer, and the host keeps it
 * out of the invitees (`cal_core::attendee::guard_create`). Only an event the
 * account organizes may notify anyone, so it carries that too. Neither is
 * ever sent to a provider.
 *
 * Typed structurally, so the desktop and the mobile event shapes both fit.
 */
export function organizerOf(source: {
  organizer?: string | null;
  organized_elsewhere?: boolean;
}): { organizer: string | null; organized_elsewhere: boolean } {
  return {
    organizer: source.organizer ?? null,
    organized_elsewhere: source.organized_elsewhere === true,
  };
}
