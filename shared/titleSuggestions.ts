// Offering what you have written before, from the title field of both editors.
//
// Most appointments and most tasks are not new — they are the same thing
// again: the physio at 45 minutes with a reminder half an hour before, the
// weekly report with its checklist in the description. Typing all of that a
// second time is work the app already knows the answer to.
//
// So the title field offers what MATCHES what is being typed, and accepting an
// offer fills the rest of the editor from that earlier item. What it does NOT
// fill is the one thing that makes this a new entry: WHEN it happens. The day
// comes from wherever the user started the editor — a tapped day, today, the
// slot they picked — and the offer must never quietly move it.
//
// Everything here is pure, so both platforms decide identically and the rules
// can be argued with in tests rather than in a running app.

/** The least a stored item needs to be offered again. */
export interface SuggestibleItem {
  id: string;
  title: string;
}

/** One offer, ready to render. */
export interface TitleSuggestion<T extends SuggestibleItem> {
  /** The earlier item this came from. */
  item: T;
  /** Its title, exactly as it was written then. */
  title: string;
}

/**
 * Fold to something two spellings of the same title agree on.
 *
 * Case and spacing carry no intent here — "Team Standup", "team standup" and
 * "Team  Standup" are one habit, and offering all three would spend the list
 * on the same answer three times. Diacritics are kept: "Grüße" and "Grusse"
 * are not the same word, and a German user typing the umlaut means it.
 */
function fold(title: string): string {
  return title.trim().toLowerCase().replace(/\s+/g, ' ');
}

/** Where the query sits in the title — earlier is a better answer. */
function rankOf(title: string, query: string): number {
  const t = fold(title);
  const q = fold(query);
  if (q === '') return -1;
  if (t === q) return 0;
  if (t.startsWith(q)) return 1;
  // A word boundary: "standup" should find "Team Standup" as readily as
  // "Standup Team", but not "Understanding" — mid-word noise is the fastest
  // way to make a suggestion list useless.
  const at = t.indexOf(q);
  if (at > 0 && /\s/.test(t[at - 1] ?? '')) return 2;
  return -1;
}

/**
 * The task statuses that mean "still to do".
 *
 * Named here because two things need the same answer: the tier that decides
 * which row of a title wins, and the SEARCH PASS that guarantees such a row is
 * in the result set at all.
 */
export const UNFINISHED_TASK_STATUSES = ['open', 'in_progress'] as const;

/**
 * Join the two passes the task suggestions run, keeping the first sighting of
 * each row.
 *
 * One pass would do if the index answered fairly, and it does not: it returns
 * the best 200 matches by relevance, and for a repeating task on a provider
 * with native recurrence the matches for one title are mostly its completion
 * records — one per tick, forever. A user with years of history can therefore
 * push their own LIVE task out of its own result set, and no amount of
 * cleverness afterwards can offer a row that never arrived.
 *
 * So the unfinished rows are asked for separately, where the finished ones
 * cannot crowd them out, and the general pass still brings the history that
 * makes one-off tasks worth offering. Order matters: the unfinished pass comes
 * first, so a row present in both keeps the identity it had there.
 */
export function joinSuggestionPasses<T extends { id: string }>(
  first: readonly T[],
  second: readonly T[],
  // What makes two rows the same row. The id alone does not: two task
  // servers count from the same 1 (see `offerKey`).
  keyOf: (item: T) => string = (item) => item.id,
): T[] {
  const seen = new Set<string>();
  const out: T[] = [];
  for (const item of [...first, ...second]) {
    const key = keyOf(item);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(item);
  }
  return out;
}

/**
 * The offers for what has been typed, best first.
 *
 * Matching is on the TITLE only. The search index behind this also covers
 * description, location and attendees — useful when looking for something,
 * wrong here: an offer whose title has nothing to do with the typed words
 * looks like the app inventing things.
 *
 * One offer per distinct title: writing the same appointment twelve times
 * should not fill the list twelve times.
 *
 * WHICH of the twelve, though, is not simply the newest — and that cost a
 * repetition. A repeating task on a provider with native recurrence (Vikunja)
 * leaves a COMPLETION RECORD behind on every tick: a finished, deliberately
 * non-repeating copy under the same title, created just now. It is therefore
 * always the most recent row of its name, so it always won, and accepting the
 * offer filled the editor from a copy whose whole purpose is to have no
 * repetition and no reminders. The living task — the one still repeating in
 * the future, the one the user meant — sat right there in the same result set.
 *
 * So `tierOf` decides first and recency only breaks ties inside a tier: a
 * caller says which of its items make better templates (lower is better), and
 * for tasks that is "not finished yet". A caller that has no such distinction
 * passes nothing and gets the old behaviour exactly.
 *
 * `recencyOf` returns whatever the caller can order by (an ISO instant, a
 * timestamp); items without one sort last but are still offered.
 */
export function rankTitleSuggestions<T extends SuggestibleItem>(
  items: readonly T[],
  query: string,
  recencyOf: (item: T) => string | null | undefined,
  limit = 6,
  tierOf: (item: T) => number = () => 0,
): TitleSuggestion<T>[] {
  if (fold(query) === '') return [];
  const best = new Map<string, { item: T; rank: number; at: number; tier: number }>();
  for (const item of items) {
    const rank = rankOf(item.title, query);
    if (rank < 0) continue;
    const key = fold(item.title);
    const at = new Date(recencyOf(item) ?? '').getTime();
    const when = Number.isFinite(at) ? at : Number.NEGATIVE_INFINITY;
    const tier = tierOf(item);
    const held = best.get(key);
    const better = !held || tier < held.tier || (tier === held.tier && when > held.at);
    if (better) best.set(key, { item, rank, at: when, tier });
  }
  return [...best.values()]
    .sort((a, b) => (a.rank !== b.rank ? a.rank - b.rank : b.at - a.at))
    .slice(0, limit)
    .map(({ item }) => ({ item, title: item.title }));
}

/** What an accepted EVENT offer fills in. Everything except when it happens. */
export interface EventPrefill {
  title: string;
  /** How long it lasts, in minutes — applied to whatever day the editor holds. */
  durationMinutes: number;
  all_day: boolean;
  location: string | null;
  description: string | null;
  /** The rule to repeat by, or null. Never the old series' exceptions. */
  rrule: string | null;
  color_label: string | null;
  reminders: R[];
  attendees: string[];
  calendar_id: string;
}

/** A reminder, as far as this module cares. */
type R = unknown;

/** The event fields this module reads. */
export interface PrefillableEvent extends SuggestibleItem {
  calendar_id: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: { rrule: string; exceptions: string[]; tzid?: string | null } | null;
  color_label: string | null;
  reminders: R[];
  attendees: string[];
}

/** RFC-5545 `UNTIL`, as an instant, or NaN when there is none. */
function untilMs(rrule: string): number {
  const m = /UNTIL=(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})Z?)?/i.exec(rrule);
  if (!m) return Number.NaN;
  const [, y, mo, d, hh, mm, ss] = m;
  return hh == null
    ? Date.UTC(+y, +mo - 1, +d, 23, 59, 59, 999)
    : Date.UTC(+y, +mo - 1, +d, +hh, +mm, +ss);
}

/**
 * Everything an earlier event can lend a new one.
 *
 * The duration travels rather than the end instant, because the new event is
 * on a different day: an end lifted verbatim would land it in the past.
 *
 * The RRULE travels, but never its EXDATEs — those name instants of the OLD
 * series, and on a new one they would punch holes in days the user never
 * touched. A rule that has already ENDED does not travel either: a
 * `COUNT`-bounded one is fine (it counts from wherever it starts), but an
 * `UNTIL` in the past would create a series with nothing in it, which reads as
 * the app having silently dropped the repetition.
 */
export function eventPrefillFrom(
  source: PrefillableEvent,
  now: Date = new Date(),
): EventPrefill {
  const startMs = new Date(source.start).getTime();
  const endMs = new Date(source.end).getTime();
  const span =
    Number.isFinite(startMs) && Number.isFinite(endMs)
      ? Math.max(0, endMs - startMs)
      : 0;
  const rrule = source.recurrence?.rrule ?? null;
  const until = rrule ? untilMs(rrule) : Number.NaN;
  return {
    title: source.title,
    durationMinutes: Math.round(span / 60_000),
    all_day: source.all_day,
    location: source.location,
    description: source.description,
    rrule: rrule && (!Number.isFinite(until) || until > now.getTime()) ? rrule : null,
    color_label: source.color_label,
    reminders: source.reminders,
    attendees: source.attendees,
    calendar_id: source.calendar_id,
  };
}

/** What an accepted TASK offer fills in. Everything except when it is due. */
export interface TaskPrefill {
  title: string;
  list_id: string;
  description: string | null;
  priority: string;
  effort: string;
  color_label: string | null;
  reminders: R[];
  /** The stored recurrence, exactly as it came — the editors convert it with
   *  their own `fromBackend`, and re-encoding it here would be a second
   *  spelling of that. */
  recurrence: unknown;
  deadline_reminder_days: number | null;
}

/** The task fields this module reads. */
export interface PrefillableTask extends SuggestibleItem {
  list_id: string;
  description: string | null;
  priority: string;
  effort: string;
  color_label: string | null;
  reminders: R[];
  recurrence: unknown;
  deadline_reminder_days: number | null;
}

/**
 * Everything an earlier task can lend a new one.
 *
 * Not its dates, not its status, and not who it was assigned to: a task copied
 * from one that was assigned to a colleague would quietly put work on their
 * plate, which is not what "fill in the rest" means to anybody.
 */
export function taskPrefillFrom(source: PrefillableTask): TaskPrefill {
  return {
    title: source.title,
    list_id: source.list_id,
    description: source.description,
    priority: source.priority,
    effort: source.effort,
    color_label: source.color_label,
    reminders: source.reminders,
    recurrence: source.recurrence,
    deadline_reminder_days: source.deadline_reminder_days,
  };
}

/**
 * Which offer a suggestion stands for: the container it lives in AND its id.
 *
 * The id alone does not name one row. Google keeps an event's id across every
 * calendar it sits in, a copy keeps its UID, and two task servers count from
 * the same 1. Accepting by bare id therefore filled the editor from whichever
 * copy the search happened to return first, not the one the offer named: the
 * hint said "Arbeit", and the editor took the copy on the top calendar.
 */
export function offerKey(container: string | null | undefined, id: string): string {
  return JSON.stringify([container ?? '', id]);
}

/** A container an offer can be filled into, as far as these rules care. */
export interface OfferContainer {
  id: string;
  name: string;
  read_only: boolean;
}

/** One offer, ready for either platform's suggestion list. */
export interface OfferOption {
  /** {@link offerKey} of the item — what accepting it hands back. */
  id: string;
  title: string;
  /** Where it comes from, saying so when that container takes nothing new. */
  hint?: string;
}

/**
 * The ranked offers as options: each keyed by {@link offerKey} and hinted with
 * its container's name. A container that cannot take a new item says so in
 * the hint (`readOnlyHint`) — the offer still fills everything else, but not
 * there, and the list is where the user decides.
 */
export function offerOptions<T extends SuggestibleItem>(
  ranked: readonly TitleSuggestion<T>[],
  containerOf: (item: T) => string | null | undefined,
  containers: readonly OfferContainer[],
  readOnlyHint: (name: string) => string,
): OfferOption[] {
  return ranked.map(({ item }) => {
    const container = containers.find((c) => c.id === containerOf(item));
    return {
      id: offerKey(containerOf(item), item.id),
      title: item.title,
      hint: container
        ? container.read_only
          ? readOnlyHint(container.name)
          : container.name
        : undefined,
    };
  });
}

/** The item an accepted option stands for — by {@link offerKey}, never by id. */
export function findOffer<T extends { id: string }>(
  items: readonly T[],
  key: string,
  containerOf: (item: T) => string | null | undefined,
): T | undefined {
  return items.find((item) => offerKey(containerOf(item), item.id) === key);
}

/**
 * Whether an offer's container can take a new item at all. A container the
 * catalog does not know cannot: the editor would have nothing to show for it.
 */
export function offerUsable(
  containers: readonly OfferContainer[],
): (container: string | null | undefined) => boolean {
  return (container) =>
    containers.some((c) => c.id === container && !c.read_only);
}

/**
 * Where a filled-in NEW item goes.
 *
 * - `offer`: the offer's own container. It is known and takes new items.
 * - `pinned`: the caller keeps its own, because the user chose it.
 * - `readOnly`, `unknown`: the offer's container cannot take it. The editor
 *   keeps its own container and has to SAY so (decision 161): the offer named
 *   one container, and quietly using another is what made the editor look as
 *   if it had picked the top entry at random.
 * - `none`: the offer names no container; there is nothing to say.
 */
export type PrefillTarget =
  | { kind: 'offer'; id: string }
  | { kind: 'pinned' }
  | { kind: 'readOnly'; name: string }
  | { kind: 'unknown' }
  | { kind: 'none' };

export function prefillTarget(
  offerContainer: string | null | undefined,
  containers: readonly OfferContainer[],
  pinned: boolean,
): PrefillTarget {
  if (pinned) return { kind: 'pinned' };
  if (!offerContainer) return { kind: 'none' };
  const known = containers.find((c) => c.id === offerContainer);
  if (!known) return { kind: 'unknown' };
  if (known.read_only) return { kind: 'readOnly', name: known.name };
  return { kind: 'offer', id: known.id };
}

/**
 * Whether a quick-add's container picker outranks the offer's container
 * (decision 160): the user moved it AND left it on something other than the
 * default it showed.
 *
 * Moving alone is not a choice. A closed select fires a change on every arrow
 * key, so a screen-reader user who only listened through the calendars and
 * came back to the top had "chosen" the top calendar, and an accepted offer
 * then landed there instead of where the offer said.
 */
export function pickedOverOffer(
  moved: boolean,
  value: string,
  shownDefault: string | null | undefined,
): boolean {
  return moved && value !== (shownDefault ?? '');
}
