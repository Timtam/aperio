// Resolve the rendered colour of a task — the mobile twin of the desktop
// `src/intl/eventColor.ts` `resolveTaskColor` (DESIGN.md §8.2), a small local
// helper (like ./eventColor) until the domain types fully consolidate into
// @aperio/shared. Chain, most specific first:
//
//   1. the task's own colour label (named);
//   2. its SECTION's colour (a section tints its colourless tasks);
//   3. its task LIST's colour (bound label's live hex, else native);
//   4. none → `null`.
//
// `labelName` is set ONLY for the task's own explicit label — the section/list
// tint is a grouping cue, not a per-task signal — so callers append it to the
// accessible label exactly as the desktop does (WCAG 1.4.1).

import type { ColorLabel, Section, Task, TaskList } from '@aperio/shared';

export interface ResolvedColor {
  hex: string | null;
  labelName: string | null;
}

/** A task list's effective hex: its bound label's live hex, else native colour. */
function resolveListColorHex(
  list: TaskList | undefined,
  labelsById: Map<string, ColorLabel>,
): string | null {
  if (list?.color_label) {
    const label = labelsById.get(list.color_label);
    if (label) return label.hex;
  }
  return list?.color?.hex ?? null;
}

/**
 * `sectionId → resolved live hex` for every section that binds a colour label,
 * so a colourless task in that section inherits it. Mirrors the desktop store's
 * `sectionColorById`. Recolouring the label recolours every bound section's
 * tasks (the hex is resolved fresh here, not stored on the task).
 */
export function sectionColorMap(
  sections: Section[],
  labelsById: Map<string, ColorLabel>,
): Map<string, string> {
  const map = new Map<string, string>();
  for (const s of sections) {
    if (s.color_label) {
      const label = labelsById.get(s.color_label);
      if (label) map.set(s.id, label.hex);
    }
  }
  return map;
}

export function resolveTaskColor(
  task: Pick<Task, 'color_label' | 'list_id' | 'section_id'>,
  listsById: Map<string, TaskList>,
  labelsById: Map<string, ColorLabel>,
  sectionColorById: Map<string, string>,
): ResolvedColor {
  if (task.color_label) {
    const label = labelsById.get(task.color_label);
    if (label) return { hex: label.hex, labelName: label.name };
  }
  if (task.section_id) {
    const hex = sectionColorById.get(task.section_id);
    if (hex) return { hex, labelName: null };
  }
  return {
    hex: resolveListColorHex(listsById.get(task.list_id), labelsById),
    labelName: null,
  };
}
