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
 * Same calendar: the old key is emptied FIRST, under the NEW signature, and the
 * new row follows. The reminder scan then finds the old key's event under its
 * new id and keeps the row already there — the later write — so the old key
 * folds away and nothing is left behind. Emptied after the new row, under the
 * old signature, it was the later write, and the scan used it to empty the
 * event's reminders.
 *
 * Another calendar: the old calendar's scan never sees the event again, so the
 * old key is retired WITHOUT a signature (`EventRemindersRepo::retire` in the
 * core): emptied, so a peer holding the old list stops firing, and never
 * repaired onto whatever else in that calendar shares the title and start.
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
  if (from.calendar_id === to.calendar_id) {
    return [
      {
        calendar_id: from.calendar_id,
        event_id: from.event_id,
        reminders: [],
        title: to.title,
        starts_at: to.starts_at,
      },
      to,
    ];
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
