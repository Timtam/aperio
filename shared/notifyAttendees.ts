/**
 * Whether an editor offers "Notify attendees" (decisions 70a and 74a).
 *
 * Only on a calendar that schedules server-side, only for an event the
 * account organizes (`organized_elsewhere` comes from the adapter), and only
 * when someone is there to tell: an invitee now, or one the edit removed from
 * the event as it was opened, who may still get a cancellation. The host
 * applies the same rule again before it writes
 * (`cal_core::attendee::guard_update`), so this decides only what the editor
 * shows, and whether it asks the host to send.
 *
 * Typed structurally, so the desktop and the mobile event shapes both fit.
 */
export function offersNotifyAttendees(input: {
  supportsScheduling: boolean;
  /** The invitees as the editor holds them now. */
  attendees: readonly string[];
  /** The event as it was opened; `null` for a new one. */
  original: {
    attendees?: readonly string[] | null;
    organized_elsewhere?: boolean;
  } | null;
}): boolean {
  const { supportsScheduling, attendees, original } = input;
  if (!supportsScheduling || original?.organized_elsewhere === true) return false;
  return attendees.length > 0 || (original?.attendees?.length ?? 0) > 0;
}
