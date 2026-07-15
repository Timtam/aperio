// Which containers may be offered as the TARGET of an item — calendars for an
// event, task lists for a task (create / edit / quick-add / move-copy). Shared
// by desktop and mobile so every target picker filters identically.

/** A container as far as target selection cares: an id + writability. */
export interface SelectableCalendar {
  id: string;
  read_only: boolean;
}

/** A task list as far as target selection cares — same shape as a calendar. */
export type SelectableTaskList = SelectableCalendar;

export interface EventCalendarFilter {
  /**
   * The ids currently CHECKED in the sidebar / catalog (visible calendars,
   * selected task lists). When passed, deselected containers drop out of the
   * picker; omit it to skip the visibility filter.
   */
  selectedIds?: ReadonlySet<string>;
  /**
   * The item's current container id, always kept in the result regardless of
   * the filters — so editing an item that lives on a hidden or read-only
   * container still shows its real target instead of a blank / wrong picker.
   */
  currentId?: string;
  /**
   * When true, deselected (hidden) but WRITABLE containers are STILL offered as
   * targets — i.e. the `selectedIds` visibility filter is skipped. Drives the
   * synced "show hidden calendars / task lists as assignment targets" setting
   * (default on). Read-only containers stay excluded either way (you can't write
   * to them), and `currentId` is always kept.
   */
  includeHidden?: boolean;
}

function selectableContainers<C extends SelectableCalendar>(
  containers: readonly C[],
  { selectedIds, currentId, includeHidden }: EventCalendarFilter,
): C[] {
  return containers.filter(
    (c) =>
      c.id === currentId ||
      (!c.read_only &&
        (includeHidden || !selectedIds || selectedIds.has(c.id))),
  );
}

/**
 * Calendars eligible as an event target: WRITABLE and (when a selection is
 * passed) VISIBLE — plus the event's `currentId` unconditionally. You can't
 * write to a read-only calendar, and offering a hidden one is confusing, so
 * neither belongs in the picker; but an event already living on such a
 * calendar must still display it when opened for edit.
 */
export function selectableEventCalendars<C extends SelectableCalendar>(
  calendars: readonly C[],
  filter: EventCalendarFilter = {},
): C[] {
  return selectableContainers(calendars, filter);
}

/**
 * Task lists eligible as a task target — the task twin of
 * `selectableEventCalendars`, with identical semantics: writable, checked in
 * the lists catalog (when `selectedIds` is passed), plus the task's own
 * `currentId` unconditionally so an existing task on a deselected/read-only
 * list still shows its real list when opened for edit.
 */
export function selectableTaskLists<L extends SelectableTaskList>(
  taskLists: readonly L[],
  filter: EventCalendarFilter = {},
): L[] {
  return selectableContainers(taskLists, filter);
}
