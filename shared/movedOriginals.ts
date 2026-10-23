// What a move says when its original stayed behind (decision 277).
//
// A move to another account writes the copy at the target and then deletes the
// original. When the provider is busy for that delete, the move stands and the
// host marks the returned row with `original_left_in`: the container the
// original is still in. The person deletes it there; moving it again would
// make a second copy at the target.

import { joinNames } from './listWords';

type Translate = (key: string, values?: Record<string, unknown>) => string;

/** A row a move returned, as both shells' Event and Task carry it. */
export interface MovedRow {
  title: string;
  original_left_in?: string | null;
}

/**
 * The sentence for the rows of one move (the item, then the subtasks that
 * followed it) whose originals stayed, or `null` when every original went.
 * `containerName` names the list or calendar by its id.
 */
export function originalKeptSentence(
  rows: readonly MovedRow[],
  containerName: (id: string) => string | undefined,
  t: Translate,
): string | null {
  const kept = rows.filter((row) => row.original_left_in);
  if (kept.length === 0) return null;
  const sourceId = kept[0].original_left_in as string;
  const source = containerName(sourceId) ?? sourceId;
  if (kept.length === 1) {
    return t('actions.originalKept', { title: kept[0].title, source });
  }
  const titles = joinNames(
    kept.map((row) => t('actions.quotedTitle', { title: row.title })),
    t,
  );
  return t('actions.originalsKept', { titles, source });
}
