/**
 * What the editors and the delete dialogs say or ask about telling a
 * meeting's attendees (decisions 70a, 74a, 76a, 80a).
 *
 * - `'none'`: there is nobody to tell, or only the organizer could: the
 *   calendar does not schedule server-side, someone else organizes the event
 *   (`organized_elsewhere`), or nobody is invited now nor was when the event
 *   was opened.
 * - `'offer'`: the provider can save silently, so the user chooses ("Notify
 *   attendees", "Remove without notifying").
 * - `'always'`: the provider mails the attendees about every saved change and
 *   about a deletion (`always_notifies_attendees`: iCloud and other RFC 6638
 *   servers, Microsoft 365). No choice is offered that the provider would not
 *   keep; a sentence says who informs them ([`notifierSentence`]).
 *
 * The host applies the same rules again before it writes
 * (`cal_core::attendee::guard_update`), so these decide what a dialog shows,
 * and whether it asks the host to send.
 *
 * Typed structurally, so the desktop and the mobile shapes both fit.
 */
export type AttendeeNotice = 'none' | 'offer' | 'always';

/** The calendar capabilities these rules read (`host_core::wire::CalendarRow`). */
export interface NoticeCalendar {
  supports_scheduling?: boolean;
  always_notifies_attendees?: boolean;
  invitations_reply_only?: boolean;
  notifier_name?: string | null;
}

/** The parts of an event these rules read. */
export interface NoticeEvent {
  attendees?: readonly string[] | null;
  organized_elsewhere?: boolean;
}

/** The notice an editor shows for the attendees it holds now. `original`
 *  is the event as it was opened (`null` for a new one): an invitee the edit
 *  removed may still be told (74a). */
export function attendeeNotice(input: {
  calendar: NoticeCalendar | null | undefined;
  attendees: readonly string[];
  original: NoticeEvent | null;
}): AttendeeNotice {
  const { calendar, attendees, original } = input;
  if (!calendar?.supports_scheduling || original?.organized_elsewhere === true) return 'none';
  const someoneToTell = attendees.length > 0 || (original?.attendees?.length ?? 0) > 0;
  if (!someoneToTell) return 'none';
  return calendar.always_notifies_attendees === true ? 'always' : 'offer';
}

/** Whether a save asks the provider to notify: always where the provider
 *  does so anyway, and where it offers the choice, as the user chose. */
export function sendsInvitations(notice: AttendeeNotice, chosen: boolean): boolean {
  return notice === 'always' || (notice === 'offer' && chosen);
}

/** The notice a delete shows: a meeting the account organizes, with
 *  invitees, on a calendar that schedules server-side. The same rule as
 *  [`attendeeNotice`], for the event as it stands. */
export function cancellationNotice(
  calendar: NoticeCalendar | null | undefined,
  event: NoticeEvent | null | undefined,
): AttendeeNotice {
  return attendeeNotice({
    calendar,
    attendees: event?.attendees ?? [],
    original: event ?? null,
  });
}

/** The i18n key and values of the sentence that says who informs the
 *  attendees, for a change (`'change'`) or a cancellation. Names the service
 *  when the adapter knows it (`notifier_name`), else "the calendar server". */
export function notifierSentence(
  calendar: NoticeCalendar | null | undefined,
  what: 'change' | 'cancellation',
): { key: string; values: Record<string, string> } {
  const base =
    what === 'change' ? 'dialogs.event.fields.notifyAlways' : 'dialogs.deleteScope.notifyAlways';
  const service = calendar?.notifier_name?.trim();
  return service ? { key: `${base}Named`, values: { service } } : { key: base, values: {} };
}

/**
 * Whether an event is an invitation the provider keeps read-only: someone
 * else organizes it, and the provider takes only this account's own reply and
 * reminders (decision 77a, `cal_core::invitation::invitation_locked`).
 *
 * The editors then show it read-only apart from those, instead of offering
 * edits the server would refuse. Both answers come from the adapter, so
 * nothing is guessed from addresses here.
 */
export function invitationLocked(
  calendar: NoticeCalendar | null | undefined,
  event: NoticeEvent | null | undefined,
): boolean {
  return calendar?.invitations_reply_only === true && event?.organized_elsewhere === true;
}

/**
 * The sentence a delete of such an invitation adds (decision 83b): removing
 * the account's copy tells the organizer it is declined.
 *
 * Shaped like [`notifierSentence`], so every dialog appends it the same way.
 */
export function declineSentence(): { key: string; values: Record<string, string> } {
  return { key: 'dialogs.deleteScope.organizerGetsDecline', values: {} };
}
