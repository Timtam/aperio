import type {
  Task,
  TaskEffort,
  TaskPriority,
  TaskStatus,
  TaskUser,
} from './types';

/**
 * Per-status glyph for the on-chip / on-row marker.
 *
 * Glyph-only — no SVG, no font dependency, no row-shift surprises across
 * locales. All four glyphs sit on a common visual spine (the circle) so the
 * user reads them as a progression rather than four unrelated symbols.
 *
 *   open        ○  empty circle
 *   in_progress ◐  half-filled circle — "started, not finished"
 *   completed   ⬤  large filled circle (U+2B24)
 *   cancelled   ⊘  slashed circle — "abandoned / no longer pursued"
 *
 * Shared across every task surface so the symbols mean the same thing wherever
 * the user sees them.
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
 * Per-priority glyph for the on-chip / on-row indicator: exclamation marks, one
 * per level — `!` low, `!!` medium, `!!!` high. The glyph carries the meaning;
 * the SR label still spells the priority out via {@link prioritySuffix}.
 */
export function priorityMarker(priority: TaskPriority): string {
  switch (priority) {
    case 'high':
      return '!!!';
    case 'low':
      return '!';
    case 'medium':
      return '!!';
  }
}

/**
 * Numeric sort rank for a priority (ascending = most urgent first): `high` → 0,
 * `medium` → 1, `low` → 2. Pair with a *stable* sort so the existing order is
 * the tiebreaker within one priority bucket.
 */
export function priorityRank(priority: TaskPriority): number {
  switch (priority) {
    case 'high':
      return 0;
    case 'medium':
      return 1;
    case 'low':
      return 2;
  }
}

/**
 * i18n key for the SR-announced priority label, or `null` for `medium` (no
 * announcement — keeps the common case unchanged).
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
 * SR-friendly priority suffix for aria-labels. Empty string for `medium`, so it
 * can be appended unconditionally to any task label. Comma-prefixed, so the
 * calling i18n template (`{{state}}{{priority}}{{progress}}`) needs no
 * conditional.
 */
export function prioritySuffix(
  t: (key: string, vars?: Record<string, unknown>) => string,
  priority: TaskPriority,
): string {
  const key = priorityI18nKey(priority);
  return key ? `, ${t(key)}` : '';
}

/**
 * i18n key for the SR-announced effort label, or `null` for `medium` (the
 * neutral default — no announcement, mirroring `priorityI18nKey`).
 */
export function effortI18nKey(effort: TaskEffort): string | null {
  switch (effort) {
    case 'small':
      return 'views.tasks.effortSmall';
    case 'large':
      return 'views.tasks.effortLarge';
    case 'medium':
      return null;
  }
}

/**
 * SR-friendly effort suffix for aria-labels (", großer Aufwand"). Empty string
 * for `medium` so it appends unconditionally. Comma-prefixed like
 * {@link prioritySuffix}. Always present regardless of the visual-sizing toggle,
 * so a screen-reader user always hears the effort.
 */
export function effortSuffix(
  t: (key: string, vars?: Record<string, unknown>) => string,
  effort: TaskEffort,
): string {
  const key = effortI18nKey(effort);
  return key ? `, ${t(key)}` : '';
}

/**
 * The chip-class modifier token for an effort's visual tile size, or '' for
 * `small` — the whole scale sits one step above the original mapping (tester
 * feedback): small renders at the neutral base size (no class), medium at the
 * former "large" size, large bigger still. Single source of truth for the
 * effort→size hook: a caller builds `${prefix}--effort-${token}` only when
 * the token is non-empty AND the user's `visualEffortSizing` pref is on.
 */
export function effortSizeModifier(effort: TaskEffort): string {
  return effort === 'small' ? '' : effort;
}

/**
 * i18n key for the SR-announced state suffix. Keys live under
 * `views.tasks.state*`. Adding a new TaskStatus value? Extend both this switch
 * and the locale files — the exhaustive switch gives a type error if a case is
 * forgotten.
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
 * Count completed children of `parentId` among `allTasks`. Returns `null` when
 * the parent has no children at all. "Done" means `completed`; `cancelled`
 * rows drop out of the total so the fraction reflects what's left to do.
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
 * SR-friendly progress suffix for aria-labels. Empty string when the task has
 * no children. Resolves through `views.tasks.subtaskProgress` which already
 * starts with a comma separator.
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

/**
 * The parent task's title when `task` is a subtask whose parent is present in
 * `allTasks`, else null. Used to label a subtask chip in context wherever it
 * surfaces on its own (calendar grid, backlog rail).
 */
export function subtaskParentTitle(
  task: Task,
  allTasks: Task[],
): string | null {
  if (!task.parent_id) return null;
  return allTasks.find((p) => p.id === task.parent_id)?.title ?? null;
}

/**
 * SR-friendly suffix naming a subtask's parent (", Unteraufgabe von X"), or ''
 * when the task isn't a subtask. Resolves through `views.tasks.subtaskParent`,
 * which starts with a comma separator — mirrors {@link subtaskProgressSuffix}.
 */
export function subtaskParentSuffix(
  t: (key: string, vars?: Record<string, unknown>) => string,
  task: Task,
  allTasks: Task[],
): string {
  const parent = subtaskParentTitle(task, allTasks);
  return parent ? t('views.tasks.subtaskParent', { parent }) : '';
}

/**
 * SR-friendly assignee suffix for aria-labels. Empty string when the task has
 * no assignees. Resolves through `views.tasks.assigneeSuffix`, which starts
 * with a comma separator. Shared across every task surface so an assignment is
 * announced wherever the task appears.
 */
export function assigneeSuffix(
  t: (key: string, vars?: Record<string, unknown>) => string,
  assignees: TaskUser[],
): string {
  if (assignees.length === 0) return '';
  return t('views.tasks.assigneeSuffix', {
    names: assignees.map((a) => a.name).join(', '),
  });
}
