// Event recurrence expansion now lives in @aperio/shared (generic over a
// minimal event shape) so the desktop + mobile apps share one implementation.
// This module re-exports it unchanged, plus the desktop-typed `ExpandedEvent`
// alias, so every existing `../intl/recurrence` import keeps working.
import type { ExpandedOccurrence } from '@aperio/shared';

import type { CalendarEvent } from '../api/types';

export {
  expandEvent,
  expandAll,
  isExpandedOccurrence,
  isSeriesOccurrence,
  seriesIdOf,
  occurrenceIsoOf,
  truncateRRuleBefore,
  splitRRuleForEdit,
  localTimeZone,
  withCreatedRecurrenceZone,
} from '@aperio/shared';

/** An expanded per-occurrence copy of the desktop `CalendarEvent`. */
export type ExpandedEvent = ExpandedOccurrence<CalendarEvent>;
