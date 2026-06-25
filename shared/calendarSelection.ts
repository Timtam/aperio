// Which calendars may be offered as the TARGET of an event (create / edit /
// quick-add / move-copy). Shared by desktop and mobile so every event-target
// picker filters identically.

/** A calendar as far as event-target selection cares: an id + writability. */
export interface SelectableCalendar {
  id: string;
  read_only: boolean;
}

export interface EventCalendarFilter {
  /**
   * Sidebar-visible calendar ids. Desktop passes the checked set so hidden
   * calendars drop out of the picker. Mobile has no per-calendar visibility
   * toggle, so it omits this and only the read-only filter applies.
   */
  selectedIds?: ReadonlySet<string>;
  /**
   * The event's current calendar id, always kept in the result regardless of
   * the filters — so editing an event that lives on a hidden or read-only
   * calendar still shows its real target instead of a blank / wrong picker.
   */
  currentId?: string;
}

/**
 * Calendars eligible as an event target: WRITABLE and (on desktop) VISIBLE in
 * the sidebar — plus the event's `currentId` unconditionally. You can't write
 * to a read-only calendar, and offering a hidden one is confusing, so neither
 * belongs in the picker; but an event already living on such a calendar must
 * still display it when opened for edit.
 */
export function selectableEventCalendars<C extends SelectableCalendar>(
  calendars: readonly C[],
  { selectedIds, currentId }: EventCalendarFilter = {},
): C[] {
  return calendars.filter(
    (c) =>
      c.id === currentId ||
      (!c.read_only && (selectedIds ? selectedIds.has(c.id) : true)),
  );
}
