/**
 * The private-reminder writes a save makes, in the order they must be sent.
 *
 * `to` is the key the saved event carries now, with the list and the
 * signature of the event it names. `from` is the key the editor opened, when
 * that key held a row and the save made the SAME event carry another key: the
 * provider minted a new id (Exchange does on every save), or the event moved
 * to another calendar. It is `null` when the old key's event lives on — an
 * occurrence carved out of its series, the tail of a split, an occurrence
 * written into its series — because that row is the other event's, and emptying
 * it would silence every occurrence the save did not touch.
 *
 * A key the event left is retired WITHOUT a signature
 * (`EventRemindersRepo::retire` in the core): emptied, so a peer holding the
 * old list loses to it and stops firing, and never repaired. With a signature,
 * the reminder scan would repoint it onto whatever event in its calendar
 * carries that title and start, where — as the later write — it would empty
 * that event's reminders. That was the event it left, under its new id; and
 * when a twin shares the title and start, the row waits until the event
 * changes or goes, and then empties the twin.
 *
 * The retired row stays, inert: the scan skips it and nothing fires from an
 * empty list. One per such save, until the log compaction learns to drop them.
 */
export interface PrivateRemindersWrite<R> {
  calendar_id: string;
  event_id: string;
  reminders: R[];
  title: string;
  starts_at: string;
}

export function privateReminderWrites<R>(input: {
  to: PrivateRemindersWrite<R>;
  from: { calendar_id: string; event_id: string } | null;
}): PrivateRemindersWrite<R>[] {
  const { to, from } = input;
  if (from == null || (from.calendar_id === to.calendar_id && from.event_id === to.event_id)) {
    return [to];
  }
  return [
    to,
    {
      calendar_id: from.calendar_id,
      event_id: from.event_id,
      reminders: [],
      title: '',
      starts_at: '',
    },
  ];
}
