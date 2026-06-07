import type { Task, TaskPriority, TaskStatus } from '../api/types';

/**
 * Per-status glyph for the on-chip / on-row marker.
 *
 * Glyph-only — no SVG, no font dependency, no row-shift surprises
 * across locales. All four glyphs sit on a common visual spine (the
 * circle) so the user reads them as a progression rather than four
 * unrelated symbols. The previous mix of boxes (☐/☑/☒) and a stray
 * circle (◐) for in_progress was confusing — a half-filled circle
 * next to an empty box doesn't tell the eye "these are positions on
 * the same scale".
 *
 *   open        ○  empty circle
 *   in_progress ◐  half-filled circle — "started, not finished"
 *   completed   ⬤  large filled circle (U+2B24) — reads clearly as a
 *                  disc rather than a stray bullet (the small ● did)
 *   cancelled   ⊘  slashed circle — "abandoned / no longer pursued"
 *
 * Shared across TaskView and the calendar-chip surfaces so the
 * symbols mean the same thing wherever the user sees them. The
 * markers render inside a centred fixed-size box on every surface, so
 * the larger "completed" glyph stays aligned and never shifts a row.
 */
export function statusMarker(status: TaskStatus): string {
  switch (status) {
    case 'completed':
      return '⬤';
    case 'cancelled':
      return '⊘';
    case 'in_progress':
      return '◐';
    case 'open':
      return '○';
  }
}

/**
 * Per-priority glyph for the on-chip / on-row indicator. One character
 * wide like {@link statusMarker}. `medium` — the default for virtually
 * every task — renders **nothing**, so only deliberately raised or
 * lowered tasks draw a glyph and the indicator stays quiet. The arrows
 * read as positions on one axis around an implied neutral middle:
 * high `↑`, low `↓`, medium = baseline (absent).
 */
export function priorityMarker(priority: TaskPriority): string {
  switch (priority) {
    case 'high':
      return '↑';
    case 'low':
      return '↓';
    case 'medium':
      return '';
  }
}

/**
 * i18n key for the SR-announced priority label, or `null` for `medium`
 * (no announcement — keeps the common case unchanged). The resolved
 * string is the bare label ("high priority"); {@link prioritySuffix}
 * adds the comma separator for inline use, while sites that need a
 * stand-alone `aria-label` can resolve the key directly.
 */
export function priorityI18nKey(priority: TaskPriority): string | null {
  switch (priority) {
    case 'high':
      return 'views.tasks.priorityHigh';
    case 'low':
      return 'views.tasks.priorityLow';
    case 'medium':
      return null;
  }
}

/**
 * SR-friendly priority suffix for aria-labels. Empty string for
 * `medium`, so it can be appended unconditionally to any task label.
 * Like {@link subtaskProgressSuffix} the result is comma-prefixed, so
 * the calling i18n template (`{{state}}{{priority}}{{progress}}`) needs
 * no conditional.
 */
export function prioritySuffix(
  t: (key: string, vars?: Record<string, unknown>) => string,
  priority: TaskPriority,
): string {
  const key = priorityI18nKey(priority);
  return key ? `, ${t(key)}` : '';
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
