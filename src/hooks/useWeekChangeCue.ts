import { useCallback, useState } from 'react';

/**
 * Whether the active day has just moved into another calendar week: the one
 * moment a grid's cell name carries the week (TODO B15). A screen reader in a
 * grid reads only the cell it lands on, so a week change would otherwise go
 * unsaid; saying the week on every step instead would bury the day.
 *
 * `activeDayKey` is the focused day, `weekKey` the first day of the week it
 * is shown in, `inGrid` whether the step happens with the focus in the grid.
 * A step taken elsewhere — the toolbar's buttons — is heard on entering the
 * grid, whose own name carries the week, so the cell does not repeat it.
 *
 * The answer holds until the active day moves again: a reload of the day's
 * items re-renders the cell, and a name that changed then would make NVDA
 * read the cell a second time. A changed week start alone, with the day
 * staying put, moves no cue. The second value drops the cue, for the grid to
 * call when the focus leaves it: coming back, the grid's name says the week
 * and the cell would say it again.
 *
 * Derived during render rather than in an effect, so the name is right in the
 * same frame that moves `aria-activedescendant`.
 */
export function useWeekChangeCue(
  activeDayKey: string,
  weekKey: string,
  inGrid: boolean,
): [cue: boolean, drop: () => void] {
  const [track, setTrack] = useState({ activeDayKey, weekKey, cue: false });
  const drop = useCallback(
    () => setTrack((current) => (current.cue ? { ...current, cue: false } : current)),
    [],
  );
  if (track.activeDayKey === activeDayKey && track.weekKey === weekKey) {
    return [track.cue, drop];
  }
  const moved = track.activeDayKey !== activeDayKey;
  const next = {
    activeDayKey,
    weekKey,
    cue: moved ? inGrid && weekKey !== track.weekKey : track.cue,
  };
  setTrack(next);
  return [next.cue, drop];
}
