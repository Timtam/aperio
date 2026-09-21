/**
 * The private-reminder write that retires a key: the event it named moved to
 * another calendar, or its provider minted a new id (Exchange does on every
 * save). The list is emptied, so a peer holding the old one stops firing, and
 * it carries NO signature, so the reminder scan's repair never takes it for
 * the same event reminted and empties the event that took over the key.
 *
 * The rule is the core's (`EventRemindersRepo::retire`); this only spells the
 * write both editors send through the existing command.
 */
export const retiredPrivateReminders = (calendarId: string, eventId: string) => ({
  calendar_id: calendarId,
  event_id: eventId,
  reminders: [],
  title: '',
  starts_at: '',
});
