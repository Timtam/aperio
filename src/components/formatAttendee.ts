import type { Contact } from '../api/types';

/** Display format for an attendee picked from a `Contact`:
 *  `"Display Name <email>"` when an email is available,
 *  display name alone when not (Aperio shows the contact name in
 *  the chip either way; the email is what the calendar sync layer
 *  later reaches for). */
export function formatAttendee(contact: Contact): string {
  const email = contact.emails[0]?.trim();
  if (email) {
    return `${contact.display_name} <${email}>`;
  }
  return contact.display_name;
}
