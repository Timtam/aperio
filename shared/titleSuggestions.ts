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
 * The offers for what has been typed, best first.
 *
 * Matching is on the TITLE only. The search index behind this also covers
 * description, location and attendees — useful when looking for something,
 * wrong here: an offer whose title has nothing to do with the typed words
 * looks like the app inventing things.
 *
 * One offer per distinct title, the most RECENT of its kind: writing the same
 * appointment twelve times should not fill the list twelve times, and the
 * latest one is the one that reflects how the habit looks now.
 *
 * `recencyOf` returns whatever the caller can order by (an ISO instant, a
 * timestamp); items without one sort last but are still offered.
 */
export function rankTitleSuggestions<T extends SuggestibleItem>(
  items: readonly T[],
  query: string,
  recencyOf: (item: T) => string | null | undefined,
  limit = 6,
): TitleSuggestion<T>[] {
  if (fold(query) === '') return [];
  const best = new Map<string, { item: T; rank: number; at: number }>();
  for (const item of items) {
    const rank = rankOf(item.title, query);
    if (rank < 0) continue;
    const key = fold(item.title);
    const at = new Date(recencyOf(item) ?? '').getTime();
    const when = Number.isFinite(at) ? at : Number.NEGATIVE_INFINITY;
    const held = best.get(key);
    if (!held || when > held.at) best.set(key, { item, rank, at: when });
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
