import type { RootStackParamList } from './types';

/**
 * The route id of an event editor that must not reuse the one already up: an
 * appointment from a file (DESIGN-file-import.md, I5), or a tapped reminder
 * (`ReminderTapRouter`). Navigating to a screen of the same name otherwise
 * reuses the focused one and replaces its params under state seeded from the
 * old ones — unsaved edits gone, the old scope kept. Every other editor keeps
 * React Navigation's default (`undefined`).
 */
export function importRouteId({
  params,
}: {
  params: RootStackParamList['EventEditor'];
}): string | undefined {
  if (params.importToken != null) return `${params.importToken}:${params.importIndex ?? ''}`;
  return params.tapToken;
}

/** The same for the task editor: a tapped reminder opens a route of its own. */
export function taskEditorRouteId({
  params,
}: {
  params: RootStackParamList['TaskEditor'];
}): string | undefined {
  return params.tapToken;
}
