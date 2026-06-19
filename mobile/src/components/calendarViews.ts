// The calendar view kinds + which native-stack route each one opens. Centralised
// (not a per-screen ternary) so adding a kind updates every screen's switcher at
// once — the ternaries this replaces silently routed an unhandled kind to their
// `else` branch. The calendar views are siblings, swapped via navigation.replace
// (a flat stack), with the anchor date carried along.

export type CalendarViewKind = 'day' | 'week' | 'month' | 'agenda' | 'year';

export const CALENDAR_VIEW_ROUTE = {
  day: 'Events',
  week: 'Week',
  month: 'Month',
  agenda: 'Agenda',
  year: 'Year',
} as const satisfies Record<CalendarViewKind, string>;
