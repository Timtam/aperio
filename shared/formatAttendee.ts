/** Display format for an attendee picked from a contact:
 *  `"Display Name <email>"` when an email is available, display name alone
 *  when not (the chip shows the contact name either way; the email is what
 *  the calendar sync layer later reaches for).
 *
 *  Typed structurally over the fields it needs so the desktop and mobile
 *  `Contact` shapes (which live in their own api layers) both satisfy it —
 *  the desktop AttendeePicker and the mobile AttendeesEditor share one
 *  source of truth. */
export function formatAttendee(contact: {
  display_name: string;
  emails: string[];
}): string {
  const email = contact.emails[0]?.trim();
  if (email) {
    return `${contact.display_name} <${email}>`;
  }
  return contact.display_name;
}
