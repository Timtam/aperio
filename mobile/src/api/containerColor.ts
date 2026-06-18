// Container colour-binding api-client (§8.2) — bind a calendar / task list /
// contact list to a global colour label. The mobile twin of the desktop
// set_container_color_label command's LOCAL branch: a local container carries
// the binding on its own row, so the Host updates it and emits the container's
// Updated sync event (→ background push). External containers + contact lists
// bind via a host-local override (desktop-only for now), so the Host rejects
// them — the UI only offers the picker for local containers.

import CalFfi from '../../modules/cal-ffi';

import { scheduleBackgroundPush } from './syncTriggers';

export type ContainerKind = 'calendar' | 'task_list' | 'contact_list';

/** Bind (or clear, with `null`) a LOCAL container's colour label. */
export const setContainerColorLabel = async (
  containerId: string,
  kind: ContainerKind,
  colorLabelId: string | null,
): Promise<void> => {
  await CalFfi.setContainerColorLabel(containerId, kind, colorLabelId);
  scheduleBackgroundPush();
};

/** Rename a LOCAL container (calendar / task list). The new name rides the
 *  container's own row + Updated sync event. External containers + contact
 *  lists (provider/override path) reject — the UI only offers it for local. */
export const renameContainer = async (
  containerId: string,
  kind: ContainerKind,
  name: string,
): Promise<void> => {
  await CalFfi.renameContainer(containerId, kind, name);
  scheduleBackgroundPush();
};
