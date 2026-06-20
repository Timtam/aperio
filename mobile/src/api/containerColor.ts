// Container colour-binding api-client (§8.2) — bind a calendar / task list /
// contact list to a global colour label. A LOCAL container carries the binding
// on its own row, so the Host updates it and emits the container's Updated sync
// event (→ background push). An EXTERNAL container / contact list binds via a
// host-LOCAL colour override (per-device, not synced) — the Host stamps it on
// read. Both routes go through the same call; the UI offers the picker for
// every container.

import CalFfi from '../../modules/cal-ffi';

import { scheduleBackgroundPush } from './syncTriggers';

export type ContainerKind = 'calendar' | 'task_list' | 'contact_list';

/** Bind (or clear, with `null`) a container's colour label — on its own row for
 *  a local container, as a host-local override for an external one. */
export const setContainerColorLabel = async (
  containerId: string,
  kind: ContainerKind,
  colorLabelId: string | null,
): Promise<void> => {
  await CalFfi.setContainerColorLabel(containerId, kind, colorLabelId);
  scheduleBackgroundPush();
};

/** Rename a container. A local calendar / task list's new name rides its own row
 *  + Updated sync event; an external one's rename is pushed to its provider
 *  (else a host-local name override). A contact list (local or external) renames
 *  its own row at the source — contacts aren't event-logged, so no sync event. */
export const renameContainer = async (
  containerId: string,
  kind: ContainerKind,
  name: string,
): Promise<void> => {
  await CalFfi.renameContainer(containerId, kind, name);
  scheduleBackgroundPush();
};

/** Set (or clear, with `null`) a SECTION's colour label. A local section binds
 *  it on its own row (+ sync event); an external section stores a host-local
 *  override. `listId` routes the call. */
export const setSectionColor = async (
  sectionId: string,
  listId: string,
  colorLabelId: string | null,
): Promise<void> => {
  await CalFfi.setSectionColor(sectionId, listId, colorLabelId);
  scheduleBackgroundPush();
};

/** Set (or clear, with `null`) an external EVENT's host-local colour override.
 *  A no-op for local / colour-capable calendars (the colour rides the event
 *  there). `eventId` is the series master id; `calendarId` routes the call. */
export const setEventColor = async (
  eventId: string,
  calendarId: string,
  colorLabelId: string | null,
): Promise<void> => {
  await CalFfi.setEventColor(eventId, calendarId, colorLabelId);
  scheduleBackgroundPush();
};
