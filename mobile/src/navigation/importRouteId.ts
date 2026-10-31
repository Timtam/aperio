import type { RootStackParamList } from './types';

/**
 * The route id of an editor opened on an appointment from a file
 * (DESIGN-file-import.md, I5): a route of its own, so a file shared or opened
 * while an editor is up never lands in that editor and replaces its params
 * under state seeded from the old ones. Every other editor keeps React
 * Navigation's default (`undefined`): navigating to it reuses the focused one.
 */
export function importRouteId({
  params,
}: {
  params: RootStackParamList['EventEditor'];
}): string | undefined {
  return params.importToken == null
    ? undefined
    : `${params.importToken}:${params.importIndex ?? ''}`;
}
