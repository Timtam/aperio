/** Day markers — what a day was like.
 *
 *  A small user-defined vocabulary, and one record per day saying which of it
 *  applied. Deliberately not tasks: a task has one status, this needs one per
 *  (marker, day), and a thing that must be excluded from every surface tasks
 *  appear on is not a task.
 *
 *  Everything here is pure. The platform layers own the calls and the widgets;
 *  what they must agree on is which markers a day resolves to, and how that
 *  reads aloud. */

/** One entry in the vocabulary. Mirrors `cal_core::DayMarker`. */
export interface DayMarker {
  id: string;
  /** Whatever the user wants it to be — a word, a sentence, an emoji. */
  name: string;
  /** Short stand-in for the dense views, typically one emoji. */
  symbol?: string | null;
  color_label?: string | null;
  position?: number;
}

/** What one day was marked with. Mirrors `cal_core::DayLog`. */
export interface DayLog {
  /** Local day key, `YYYY-MM-DD`. */
  day: string;
  markers: string[];
  /** Reserved for the "how was today" scale this design deferred. */
  rating?: number | null;
  /** REQUIRED, even though both write boundaries overwrite it with their own
   *  clock: `cal_core::DayLog::updated_at` carries no serde default, so a log
   *  without one is rejected before it ever reaches the code that would have
   *  replaced it. Optional here once meant every construction site could mint
   *  a payload the backend refused. */
  updated_at: string;
}

/** An untouched day. Reads exactly like a stored day with nothing on it, so
 *  callers never branch on "was there a record" — including on the way BACK to
 *  the store, which is why this carries a timestamp it does not need. */
export function emptyDayLog(day: string, now = new Date()): DayLog {
  return { day, markers: [], updated_at: now.toISOString() };
}

/** Whether a day says anything at all — the test every summary runs first. */
export function dayLogIsEmpty(log: DayLog | null | undefined): boolean {
  return !log || (log.markers.length === 0 && log.rating == null);
}

/**
 * The markers a day resolves to, in the vocabulary's own order.
 *
 * Ids that no longer resolve are DROPPED rather than rendered as a gap: that
 * is what makes a deleted marker disappear from history without anything
 * having to rewrite it. The order comes from the vocabulary, not from the
 * stored array, so a day always reads in the order the user arranged their
 * markers in — not the order they happened to tick them that morning.
 */
export function resolveDayMarkers(
  log: DayLog | null | undefined,
  vocabulary: readonly DayMarker[],
): DayMarker[] {
  if (!log || log.markers.length === 0) return [];
  const ticked = new Set(log.markers);
  return vocabulary.filter((m) => ticked.has(m.id));
}

/**
 * Whether two vocabularies are in the same order.
 *
 * How a caller asks "did that move change anything" — NOT `===`, which can
 * never be true here because both reorder helpers sort first and sorting
 * allocates. The identity check that used to stand in for this was dead code:
 * a "move up" on the first row still flipped the busy flag and announced a new
 * position, for a move that had not happened.
 */
export function sameDayMarkerOrder(
  a: readonly DayMarker[],
  b: readonly DayMarker[],
): boolean {
  return a.length === b.length && a.every((m, i) => m.id === b[i].id);
}

/** Tick or untick one marker, returning the day as it now stands. */
export function toggleDayMarker(log: DayLog, id: string): DayLog {
  const has = log.markers.includes(id);
  return {
    ...log,
    markers: has ? log.markers.filter((m) => m !== id) : [...log.markers, id],
  };
}

/**
 * The compact visual form: the markers' symbols run together.
 *
 * Sighted-only by design — it is decoration, and every caller that renders it
 * must hide it from the accessibility tree and put {@link spokenDaySummary}
 * on the surrounding element instead. A marker with no symbol contributes
 * nothing here rather than a fallback letter: a row of initials is noise, and
 * the spoken form carries the full truth anyway.
 */
export function compactDaySummary(
  log: DayLog | null | undefined,
  vocabulary: readonly DayMarker[],
): string {
  return resolveDayMarkers(log, vocabulary)
    .map((m) => m.symbol?.trim())
    .filter((s): s is string => !!s)
    .join(' ');
}

/**
 * How a day's markers read aloud, or `null` when the day says nothing.
 *
 * NAMES, never symbols: an emoji is announced by whatever name the screen
 * reader happens to have for it ("man running"), which is not what the user
 * called it. The name is what they typed.
 *
 * Callers append this to the day heading's own accessible name rather than
 * rendering it as its own focus stop — the requirement was an overview from
 * every view, and paying a swipe per day for it would be the opposite.
 */
export function spokenDaySummary(
  log: DayLog | null | undefined,
  vocabulary: readonly DayMarker[],
  lead: string,
): string | null {
  const names = resolveDayMarkers(log, vocabulary).map((m) => m.name.trim());
  if (names.length === 0) return null;
  return `${lead}: ${names.join(', ')}`;
}

/** Index a range of days by their key, for views that fetched a whole month
 *  in one call and now render day by day. */
export function dayLogsByDay(logs: readonly DayLog[]): Map<string, DayLog> {
  return new Map(logs.map((log) => [log.day, log]));
}

/**
 * The vocabulary in the user's order.
 *
 * `position` then name: two markers can share a position (a reorder that
 * raced, or a sync from a device mid-edit), and a stable second key keeps the
 * list from shuffling under the reader between two loads. Mirrors the ORDER BY
 * the store already applies, so a frontend that sorts a locally-mutated list
 * agrees with the next read from disk.
 */
export function sortDayMarkers(markers: readonly DayMarker[]): DayMarker[] {
  return [...markers].sort(
    (a, b) =>
      (a.position ?? 0) - (b.position ?? 0) ||
      a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
  );
}

/**
 * The vocabulary with one marker moved by `delta` places.
 *
 * Returns the whole list with `position` renumbered from zero, because that is
 * what the caller has to write back: positions are a total order, and patching
 * only the two that swapped leaves gaps that a later insert lands in the
 * middle of. An out-of-range move comes back in the ORDER it went in, so the
 * callers can offer "move up" on the first row without special-casing it —
 * test that with {@link sameDayMarkerOrder}, never with `===`: this always
 * allocates.
 */
export function moveDayMarker(
  markers: readonly DayMarker[],
  id: string,
  delta: number,
): DayMarker[] {
  const ordered = sortDayMarkers(markers);
  const from = ordered.findIndex((m) => m.id === id);
  if (from < 0) return ordered;
  return reorderDayMarkers(ordered, id, from + delta);
}

/**
 * The vocabulary with one marker moved TO a given index.
 *
 * What a drag-and-drop drop needs, where the gesture names a destination rather
 * than a distance. Same contract as {@link moveDayMarker} otherwise: the whole
 * list comes back renumbered from zero, and an index outside the list (or a
 * drop back onto the row's own slot) comes back in the order it went in.
 */
export function reorderDayMarkers(
  markers: readonly DayMarker[],
  id: string,
  toIndex: number,
): DayMarker[] {
  const ordered = sortDayMarkers(markers);
  const from = ordered.findIndex((m) => m.id === id);
  if (from < 0) return ordered;
  if (toIndex < 0 || toIndex >= ordered.length || toIndex === from) return ordered;
  const moved = ordered.slice();
  const [item] = moved.splice(from, 1);
  moved.splice(toIndex, 0, item);
  return moved.map((m, i) => ({ ...m, position: i }));
}
