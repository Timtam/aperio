// The task → calendar-day bucketing helpers now live in `@aperio/shared` (reused
// by the mobile Week/Day views); re-exported here so existing `../intl/taskDay`
// imports across the desktop are unchanged.
export {
  filterTasksOnDay,
  groupTasksByDay,
  isDeadlineChip,
  taskEndTimeOnDay,
  taskTimeOnDay,
  todayIsoKey,
  mergeDayItems,
  expandScheduledRecurringTasks,
  isRecurringProjection,
  recurringSeriesTaskId,
} from '@aperio/shared';
export type { DayGridItem } from '@aperio/shared';
