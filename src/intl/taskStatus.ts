import type { TaskStatus } from '../api/types';

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
