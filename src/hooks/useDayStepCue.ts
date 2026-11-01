import { useCallback, useState } from 'react';

/** What the step that landed on the active day changed, for its name. */
export interface DayStepCue {
  /**
   * The step happened with the focus in the grid or list, and nothing has
   * moved since: the active cell or entry names what changed. A step taken
   * elsewhere — the toolbar's buttons — is heard on entering the widget, or
   * through the view's own announcement, so the name does not repeat it.
   */
  inWidget: boolean;
  /** The step crossed into another calendar week. */
  crossedWeek: boolean;
}

/**
 * What the last step onto the active day changed (TODO B15). A screen reader
 * in a grid or list reads only what it lands on, so a new week, or in the day
 * view a new day, would otherwise go unsaid; saying it on every step instead
 * would bury the entry. The week view's cell names the week when `inWidget`
 * and `crossedWeek`; the day view's active entry names the date when
 * `inWidget`, and the week with it when `crossedWeek`.
 *
 * `activeDayKey` is the day the widget stands on, `weekKey` the first day of
 * the week it is shown in, `inWidget` whether the step happens with the focus
 * in the widget — read at render, the moment the step lands: the arrow keys
 * and Ctrl+→ step with the focus there, the toolbar's buttons from outside.
 *
 * The cue holds until the active day moves again: a reload of the items
 * re-renders the name, and a name that changed then would make NVDA read it
 * a second time. A changed week start alone, with the day staying put, moves
 * no cue. The second value drops `inWidget`, for the widget to call when the
 * focus leaves it (coming back, its own name says the week) or, in a list,
 * when the user moves to another entry.
 *
 * Derived during render rather than in an effect, so the name is right in the
 * same frame that moves `aria-activedescendant`.
 */
export function useDayStepCue(
  activeDayKey: string,
  weekKey: string,
  inWidget: boolean,
): [cue: DayStepCue, drop: () => void] {
  const [track, setTrack] = useState({
    activeDayKey,
    weekKey,
    cue: { inWidget: false, crossedWeek: false },
  });
  const drop = useCallback(
    () =>
      setTrack((current) =>
        current.cue.inWidget
          ? { ...current, cue: { ...current.cue, inWidget: false } }
          : current,
      ),
    [],
  );
  if (track.activeDayKey === activeDayKey && track.weekKey === weekKey) {
    return [track.cue, drop];
  }
  const moved = track.activeDayKey !== activeDayKey;
  const next = {
    activeDayKey,
    weekKey,
    cue: moved
      ? { inWidget, crossedWeek: weekKey !== track.weekKey }
      : track.cue,
  };
  setTrack(next);
  return [next.cue, drop];
}
