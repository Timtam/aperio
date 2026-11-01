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

const NO_CUE: DayStepCue = { inWidget: false, crossedWeek: false };

/**
 * What the last step onto the active day changed (TODO B15). A screen reader
 * in a grid or list reads only what it lands on, so a new week, or in the day
 * view a new day, would otherwise go unsaid; saying it on every step instead
 * would bury the entry. The week view's cell names the week when `inWidget`
 * and `crossedWeek`; the day view's active entry names the date when
 * `inWidget`, and the week with it when `crossedWeek`.
 *
 * `activeKey` is what the widget stands on — a day in the grids and the day
 * view, an entry in the agenda, where a second meeting on the same day is a
 * step within the week — and '' while it stands on nothing (a list that is
 * empty or still loading): that is no step, and the next entry is compared
 * with the last one there was. `weekKey` is the first day of the week the
 * active thing is shown in, `inWidget` whether the step happens with the
 * focus in the widget — read at render, the moment the step lands: the arrow
 * keys and Ctrl+→ step with the focus there, the toolbar's buttons from
 * outside.
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
  activeKey: string,
  weekKey: string,
  inWidget: boolean,
): [cue: DayStepCue, drop: () => void] {
  const [track, setTrack] = useState({
    activeKey,
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
  if (activeKey === '') return [NO_CUE, drop];
  if (track.activeKey === activeKey && track.weekKey === weekKey) {
    return [track.cue, drop];
  }
  const moved = track.activeKey !== activeKey;
  const next = {
    activeKey,
    weekKey,
    cue: moved
      ? { inWidget, crossedWeek: weekKey !== track.weekKey }
      : track.cue,
  };
  setTrack(next);
  return [next.cue, drop];
}
