// A stable, collision-free list/React key for a calendar event row.
//
// The same event can be present in SEVERAL of a user's subscribed calendars —
// most sharply with Google, which reuses one event `id` across every attendee's
// copy (and shared-calendar copies), so N calendars return N rows carrying the
// BYTE-IDENTICAL cal-core `id`. They differ only in `calendar_id`.
//
// Keying a calendar-view row by `id` alone therefore produces DUPLICATE React
// keys among siblings. React can't reconcile duplicate-keyed children when the
// sibling set changes across re-renders (day-to-day navigation, a background
// refresh, a calendar toggle): it orphans DOM nodes that are never removed, so
// they PILE UP while the view stays mounted — the "same event appears more and
// more times, fluctuating, until you switch views or restart" bug. Keying by
// `(calendar_id, id)` gives every real row a unique, stable key and eliminates it.
//
// Shared with the mobile calendar list keyExtractors so both platforms key rows
// identically.

// Separator between calendar id and event id. No provider's calendar id or event
// id contains a space (they are URLs, emails, UUIDs, hrefs, or `{href}|{uid}` /
// `{master}@{iso}` composites), so two keys collide only for the genuinely same
// (calendar, event) — which is a single row anyway.
const SEP = ' ';

/** The minimal event shape needed to key a row: its id plus owning calendar. */
export interface EventKeyFields {
  id: string;
  calendar_id: string;
}

/** Unique per (calendar, event occurrence) — use as the React `key` / FlatList
 *  key for any calendar-view event row. `id` already encodes the occurrence for
 *  expanded recurring events (`{master}@{iso}`), so this is unique per rendered
 *  instance. */
export function eventInstanceKey(ev: EventKeyFields): string {
  return `${ev.calendar_id}${SEP}${ev.id}`;
}
