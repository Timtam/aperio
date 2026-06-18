// Resolve the rendered colour of a calendar event — the mobile twin of the
// desktop `src/intl/eventColor.ts` `resolveEventColor` (DESIGN.md §8.2), kept as
// a small local helper (like ./describeDue) until the Calendar/CalendarEvent
// composite types hoist into @aperio/shared. Priority, most specific first:
//
//   1. the event's own colour label (looked up in the palette) — named;
//   2. an unmapped native colour (`color_hex`): a per-event colour a provider
//      stored that the host couldn't match to a label (an iCal feed's colour, a
//      foreign CalDAV colour) — rendered directly, unnamed;
//   3. the owning calendar's colour (its bound label's live hex, else native);
//   4. none → `null` (no swatch drawn).
//
// `labelName` is set ONLY for the event's own explicit label (cases 2/3 are a
// non-critical cue), so the view appends it to the accessible label and colour
// isn't the only signal (WCAG 1.4.1) — exactly as the desktop does.

import type { ColorLabel } from '@aperio/shared';

import type { Calendar, CalendarEvent } from '../api/calendar';

export interface ResolvedColor {
  hex: string | null;
  labelName: string | null;
}

/** A calendar's effective hex: its bound label's live hex, else native colour. */
function resolveContainerColorHex(
  calendar: Calendar | undefined,
  labelsById: Map<string, ColorLabel>,
): string | null {
  if (calendar?.color_label) {
    const label = labelsById.get(calendar.color_label);
    if (label) return label.hex;
  }
  return calendar?.color?.hex ?? null;
}

export function resolveEventColor(
  event: Pick<CalendarEvent, 'color_label' | 'color_hex' | 'calendar_id'>,
  calendarsById: Map<string, Calendar>,
  labelsById: Map<string, ColorLabel>,
): ResolvedColor {
  if (event.color_label) {
    const label = labelsById.get(event.color_label);
    if (label) return { hex: label.hex, labelName: label.name };
  }
  if (event.color_hex) return { hex: event.color_hex, labelName: null };
  const calendar = calendarsById.get(event.calendar_id);
  return { hex: resolveContainerColorHex(calendar, labelsById), labelName: null };
}
