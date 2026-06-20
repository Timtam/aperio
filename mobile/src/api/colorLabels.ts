// Colour-label api-client — the app-wide palette (§8), the mobile twin of the
// desktop color_labels commands. Labels live only in local SQLite (no external
// provider has the concept) and every mutation syncs across devices. Named
// labels are managed in the ColorLabels panel; ad-hoc labels are hidden one-off
// colours composed via the custom-colour picker (deduped by hex). The hex is
// `#rrggbb`. JSON passthrough over the cal-ffi bridge into the shared type.

import type { ColorLabel } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';

export type { ColorLabel };

/** All colour labels (named + ad-hoc). The palette UI filters out `ad_hoc`. */
export const listColorLabels = async (): Promise<ColorLabel[]> =>
  JSON.parse(await CalFfi.listColorLabelsJson()) as ColorLabel[];

/** Create a named colour label; returns the created label. */
export const createColorLabel = async (
  name: string,
  hex: string,
): Promise<ColorLabel> =>
  JSON.parse(await CalFfi.createColorLabelJson(name, hex)) as ColorLabel;

/** Resolve a one-off `hex` to a hidden ad-hoc label (deduped by hex), creating
 *  one if needed. Used by the custom-colour picker for un-named colours. */
export const getOrCreateAdHocColorLabel = async (
  hex: string,
): Promise<ColorLabel> =>
  JSON.parse(await CalFfi.getOrCreateAdHocColorLabelJson(hex)) as ColorLabel;

/** Update a colour label (rename / recolour); returns the updated label. */
export const updateColorLabel = async (
  label: ColorLabel,
): Promise<ColorLabel> =>
  JSON.parse(await CalFfi.updateColorLabelJson(JSON.stringify(label))) as ColorLabel;

/** Delete a colour label by id. Entities still bound to it resolve to no colour. */
export const deleteColorLabel = (id: string): Promise<void> =>
  CalFfi.deleteColorLabel(id);
