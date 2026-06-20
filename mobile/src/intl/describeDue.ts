import type { Task } from '@aperio/shared';

/**
 * Localized due/date description for a task — the `{{due}}` slot of
 * `views.tasks.optionLabel` and the row's visible meta line. Ported from the
 * desktop's `TaskView.describeDue`.
 *
 * Precedence (highest first):
 *   completed + `completed_at`  → "Completed: <date>"
 *   `resurface_date` > today    → "Resurfaces: <date>"  (the Upcoming group)
 *   `scheduled_date`            → "Scheduled: <date>"
 *   `deadline_date`             → "Due: <date>"
 *   otherwise                   → "No date"
 *
 * The result is NEVER empty (optionLabel always interpolates `{{due}}`).
 * `formatDate` turns a `YYYY-MM-DD` / RFC-3339 string into a localized,
 * time-free date — on mobile via `Intl.DateTimeFormat` (no date-fns), the one
 * intentional, locale-correct divergence from the desktop's date-fns `PP`.
 */
export function describeDue(
  task: Task,
  t: (key: string, vars?: Record<string, unknown>) => string,
  today: string,
  formatDate: (iso: string) => string,
): string {
  if (task.status === 'completed' && task.completed_at) {
    return t('views.tasks.completedAt', { date: formatDate(task.completed_at) });
  }
  if (task.resurface_date && task.resurface_date > today) {
    return t('views.tasks.resurfacesOn', { date: formatDate(task.resurface_date) });
  }
  if (task.scheduled_date) {
    return t('views.tasks.dueScheduled', { date: formatDate(task.scheduled_date) });
  }
  if (task.deadline_date) {
    return t('views.tasks.dueDeadline', { date: formatDate(task.deadline_date) });
  }
  return t('views.tasks.dueNone');
}
