import type {
  Calendar,
  CalendarEvent,
  ColorLabel,
  ContactList,
  ContainerColor,
  Task,
  TaskList,
} from '../api/types';

/** A container (calendar / task list / address book) for color purposes:
 *  its own native color plus an optional bound color-label. */
type ColoredContainer = {
  color: ContainerColor | null;
  color_label: string | null;
};

/**
 * The effective hex of a CONTAINER: its bound color-label's *live* hex
 * (so recoloring the label recolors the container), else its native
 * provider color, else `null`. Used both as the fallback for an item's
 * color and directly wherever a container's own swatch is drawn.
 */
export function resolveContainerColorHex(
  container: ColoredContainer | undefined,
  labelsById: Map<string, ColorLabel>,
): string | null {
  if (container?.color_label) {
    const label = labelsById.get(container.color_label);
    if (label) return label.hex;
  }
  return container?.color?.hex ?? null;
}

/**
 * Resolve the rendered color of an event or task per the priority rules
 * from DESIGN.md section 8.2:
 *
 *   1. Item-level color label (highest priority) — looked up in
 *      `labelById`.
 *   2. Unmapped native color (`color_hex`): a per-event color a
 *      color-capable provider stored (RFC 7986 COLOR) that the host
 *      couldn't match to a known label — a subscribed iCal feed's color,
 *      or a foreign color another client set on a CalDAV event. Rendered
 *      directly; display-only and unnamed.
 *   3. Container color (calendar or task list).
 *   4. Fallback to a neutral CSS variable handled at the style layer
 *      (the helper returns `null` in that case).
 *
 * The optional `labelName` is what the view should append to the
 * accessible label so colour isn't the only signal (WCAG 1.4.1).
 */
export interface ResolvedColor {
  hex: string | null;
  labelName: string | null;
}

export function resolveEventColor(
  event: Pick<CalendarEvent, 'color_label' | 'color_hex' | 'calendar_id'>,
  calendarsById: Map<string, Calendar>,
  labelsById: Map<string, ColorLabel>,
): ResolvedColor {
  if (event.color_label) {
    const label = labelsById.get(event.color_label);
    if (label) {
      return { hex: label.hex, labelName: label.name };
    }
  }
  // Native color the host couldn't map to a known label (read-only feed
  // color, or a foreign CalDAV color). Render it directly — there's no label
  // to name, so it stays a non-critical cue like the container fallback.
  if (event.color_hex) {
    return { hex: event.color_hex, labelName: null };
  }
  const calendar = calendarsById.get(event.calendar_id);
  return { hex: resolveContainerColorHex(calendar, labelsById), labelName: null };
}

/**
 * Resolve a task's rendered color. Chain (most specific first):
 *
 *   1. the task's own color label;
 *   2. the color of its **section** (`sectionColorById`, keyed by
 *      `section_id`) — a section tints its colorless tasks, so moving a
 *      colorless task to another section re-resolves to that section's
 *      color with no write;
 *   3. the task list's color;
 *   4. `null` → neutral CSS var at the style layer.
 *
 * `sectionColorById` carries each section's already-resolved *live* hex
 * (derived in the store from the section's bound label), so recoloring
 * the label recolors every bound section's tasks. `labelName` is set only
 * for the task's own explicit label — the section/list tint is a grouping
 * cue, not a per-task signal — matching how the list fallback behaves.
 */
export function resolveTaskColor(
  task: Pick<Task, 'color_label' | 'list_id' | 'section_id'>,
  listsById: Map<string, TaskList>,
  labelsById: Map<string, ColorLabel>,
  sectionColorById: Map<string, string>,
): ResolvedColor {
  if (task.color_label) {
    const label = labelsById.get(task.color_label);
    if (label) {
      return { hex: label.hex, labelName: label.name };
    }
  }
  if (task.section_id) {
    const sectionHex = sectionColorById.get(task.section_id);
    if (sectionHex) {
      return { hex: sectionHex, labelName: null };
    }
  }
  const list = listsById.get(task.list_id);
  return { hex: resolveContainerColorHex(list, labelsById), labelName: null };
}

/**
 * Resolve a container's own displayed color (its bound label's live hex,
 * else native color) plus the bound label's name for accessible labels.
 * For the sidebar swatch / panel rows that render a container directly.
 */
export function resolveContainerColor(
  container: (ColoredContainer & { color_label: string | null }) | undefined,
  labelsById: Map<string, ColorLabel>,
): ResolvedColor {
  if (container?.color_label) {
    const label = labelsById.get(container.color_label);
    if (label) return { hex: label.hex, labelName: label.name };
  }
  return { hex: container?.color?.hex ?? null, labelName: null };
}

/** Narrowing alias so callers can pass any container type. */
export type AnyContainer = Calendar | TaskList | ContactList;

/**
 * Convenience: build a lookup table from a label list. Views call this
 * once per render — cheap, since the label list is small.
 */
export function labelsLookup(labels: ColorLabel[]): Map<string, ColorLabel> {
  const map = new Map<string, ColorLabel>();
  labels.forEach((l) => map.set(l.id, l));
  return map;
}
