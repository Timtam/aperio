import type { Task, TaskStatus } from '../api/types';

/**
 * Per-status glyph for the on-chip / on-row marker.
 *
 * One character wide on every platform — no SVG, no font dependency,
 * no row-shift surprises across locales. The glyphs are chosen so
 * that the "still has work" states use boxes (✅ familiar checkbox
 * convention) and the "done with" states use box-with-content. The
 * in-progress glyph is a circle on purpose: visually distinct from
 * the boxes so a sighted scan can spot in-flight rows immediately.
 *
 *   open        ☐  empty box
 *   in_progress ◐  half-filled circle — "started, not finished"
 *   completed   ☑  checked box
 *   cancelled   ☒  X-marked box — "done with, but not successfully"
 *
 * Shared across TaskView and the calendar-chip surfaces so the
 * symbols mean the same thing wherever the user sees them.
 */
export function statusMarker(status: TaskStatus): string {
  switch (status) {
    case 'completed':
      return '☑';
    case 'cancelled':
      return '☒';
    case 'in_progress':
      return '◐';
    case 'open':
      return '☐';
  }
}

/**
 * i18n key for the SR-announced state suffix. Keys live under
 * `views.tasks.state*` so TaskView's existing strings work without
 * a rename; chip aria-labels in WeekView / DayView consume the same
 * keys via this helper.
 *
 * Adding a new TaskStatus value? Extend both this switch and the
 * locale files — the helper's signature gives an exhaustiveness
 * error from the type checker if a case is forgotten.
 */
export function statusI18nKey(status: TaskStatus): string {
  switch (status) {
    case 'completed':
      return 'views.tasks.stateDone';
    case 'cancelled':
      return 'views.tasks.stateCancelled';
    case 'in_progress':
      return 'views.tasks.stateInProgress';
    case 'open':
      return 'views.tasks.stateOpen';
  }
}

/**
 * Count completed children of `parentId` among `allTasks`.
 *
 * Returns `null` when the parent has no children at all — the caller
 * decides whether "0/0" or "no progress info" is the right
 * presentation. (TaskView hides the badge in that case; chip aria-
 * labels also skip the progress segment.) "Done" here means
 * `completed`; `cancelled` rows count as the user walking away and
 * are not progress — but they still drop out of the total so the
 * fraction reflects what's left to do, not historical noise.
 *
 * Returning `{ done, total }` rather than a string keeps the
 * formatting decision at the call site (visible "1/3" badge vs SR
 * "1 of 3 done" sentence both pull from the same numbers).
 */
export function subtaskProgress(
  parentId: string,
  allTasks: Task[],
): { done: number; total: number } | null {
  let done = 0;
  let total = 0;
  for (const t of allTasks) {
    if (t.parent_id !== parentId) continue;
    if (t.status === 'cancelled') continue;
    total += 1;
    if (t.status === 'completed') done += 1;
  }
  if (total === 0) return null;
  return { done, total };
}

/**
 * SR-friendly progress suffix for aria-labels. Empty string when the
 * task has no children — so it can be appended unconditionally to
 * any chip label that ends with `{{progress}}`. Resolves through
 * `views.tasks.subtaskProgress` which already starts with a comma
 * separator, so the calling i18n template doesn't need a conditional.
 */
export function subtaskProgressSuffix(
  t: (key: string, vars?: Record<string, unknown>) => string,
  parentId: string,
  allTasks: Task[],
): string {
  const progress = subtaskProgress(parentId, allTasks);
  if (!progress) return '';
  return t('views.tasks.subtaskProgress', progress);
}
