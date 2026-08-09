// What a home-screen widget shows, frozen into one small JSON document.
//
// A widget runs in its own process, outside the app sandbox, with no access to
// the React Native layer and a memory budget that rules out opening the
// database and re-deriving anything. So the APP does the deriving — at exactly
// the moments it already recomputes the reminder schedule — and leaves the
// answer in a file the widget only has to decode.
//
// That is not a downgrade in freshness: nothing writes the database except the
// app itself and its background-sync task, and both run this. A widget reading
// the database live would see the same bytes, having paid for a second copy of
// the engine to get there.
//
// The snapshot is deliberately a LIST reaching into the future, not "the next
// item". The widget re-renders on the system's schedule, long after this ran,
// and has to answer "what is next" at a time nobody knew when it was written.

import { localDateKey } from './dateKey';
import {
  expandScheduledRecurringTasks,
  isRecurringProjection,
} from './expandTaskOccurrences';
import { collapseEventGroups } from './collapseEventGroups';
import type { EventGroup } from './eventGroups';
import { expandAll, seriesIdOf, type RecurringEventLike } from './recurrence';
import { filterTasksOnDay, taskTimeOnDay } from './taskDay';
import type { Task, TaskUser } from './types';

/** Bumped when the shape changes incompatibly. The widget refuses a version it
 *  does not know rather than rendering guesses — an app and its extension are
 *  updated together, but only the app's process restarts promptly. */
export const WIDGET_SNAPSHOT_VERSION = 1;

/** One row a widget can render. Flat and small on purpose: every field costs
 *  bytes in a file that is read on a battery budget. */
export interface WidgetItem {
  kind: 'event' | 'task';
  /** The item's id — an expanded occurrence carries its occurrence id, which is
   *  what a deep link or a tick-off has to address. */
  id: string;
  title: string;
  /** RFC-3339 UTC instant this row sorts and counts down to. For an untimed
   *  item it is the START of its local day, so "today, no time" sorts ahead of
   *  today's timed rows rather than vanishing among them. */
  at: string;
  /** Events only: RFC-3339 end, so the widget can tell running from upcoming. */
  end?: string;
  /** No clock time of its own: an all-day event, or a task with only a date. */
  untimed: boolean;
  /** Owning calendar / task list. Lets the widget group, and a later tick-off
   *  address the right list. */
  containerId: string;
  /** `#rrggbb`, when the item resolves to one. For the sighted UI only — never
   *  the sole carrier of meaning. */
  color?: string;
  /** A task's current state, `open` or `in_progress`. Absent on events.
   *
   *  Completed and cancelled never reach the snapshot, so those two are the
   *  whole range. Carried because a widget row that shows only a title cannot
   *  say whether the thing is untouched or already underway — and with the
   *  cycling check-off mode, that is also what decides what one tap does. */
  status?: 'open' | 'in_progress';
  /** The widget may offer to tick this row off.
   *
   *  Decided HERE, not in the extension, because the rules are the app's: only
   *  tasks, and never a recurring PROJECTION — a future occurrence is a preview,
   *  and completion belongs on the current instance, which is what advances the
   *  series. The app's own lists apply exactly this rule; the widget must not
   *  offer an action the app would refuse. */
  completable?: boolean;
}

/** The handful of words a widget has to say that are not data.
 *
 *  They travel WITH the snapshot rather than living in the extension, because
 *  the language is the one the user picked IN THE APP — which can differ from
 *  the device locale, and which an extension has no way to read. Times and dates
 *  are deliberately NOT in here: those the widget formats from the instant, so
 *  they follow the phone's regional settings the way every other date on the
 *  home screen does. */
export interface WidgetStrings {
  /** The window holds nothing. "Nichts geplant." */
  empty: string;
  /** The window holds nothing WITH A CLOCK TIME — the empty state of the
   *  countdown widget, which shows only timed items. Distinct from `empty`
   *  because a running all-day event is not "nothing planned", and saying so
   *  while someone is on holiday would be plainly false.
   *  "Nichts mit Uhrzeit." */
  noTimed: string;
  /** The snapshot is older than its own horizon, so an empty list can no longer
   *  be trusted to mean "nothing planned". "Keine aktuellen Daten." */
  stale: string;
  /** An item with no clock time. "Ganztägig" */
  allDay: string;
  /** The current day, so a row with no clock time still answers "when".
   *  "Heute" — the widget cannot spell this itself without a calendar of the
   *  app's language. */
  today: string;
  /** An event that has already started, as a TEMPLATE with a `{time}`
   *  placeholder: "Läuft bis {time}".
   *
   *  A template rather than a finished sentence because WHICH item is running
   *  depends on when the widget renders, which is hours after this was written —
   *  but the wording still has to come from the app's language. The widget
   *  substitutes the end time in the phone's regional format. */
  runningUntil: string;
  /** "Termin" / "Aufgabe". A row's title often does not say which it is, and
   *  nothing else on the widget does either — a colour dot certainly does not.
   *  Spoken at the END of a row, after the identifying content.
   *
   *  `kindTask` is the fallback for a task row carrying no status; a task that
   *  has one says its state instead, which implies the kind. */
  kindEvent: string;
  kindTask: string;
  /** "Offen" / "In Arbeit" — a task's state, spoken where an event says its
   *  kind. Both are named rather than one being signalled by silence: on a
   *  surface with no legend, "no word here" is a convention the listener has to
   *  have been told. */
  statusOpen: string;
  statusInProgress: string;
}

export interface WidgetSnapshot {
  version: number;
  /** When the app produced this, RFC-3339. The widget shows its age when the
   *  snapshot has gone stale — a blank widget must never be ambiguous between
   *  "nothing planned" and "nothing known". */
  generatedAt: string;
  /** The end of the window this covers, RFC-3339. Past it the widget knows it
   *  has run out of data rather than run out of appointments. */
  horizonEnd: string;
  /** The language Aperio is running in, as a BCP-47 tag ("de", "en").
   *
   *  A widget extension cannot work this out for itself, and its two obvious
   *  guesses are both wrong. `Locale.current` is intersected with the
   *  localizations the BUNDLE declares — and an extension with no `.lproj`
   *  folders declares none, so it falls back to the development language and
   *  says "in 17 hours" on a German phone. The device's preferred language is
   *  closer but still not it: Aperio's language can be overridden in its own
   *  settings, and the widget should follow the app it belongs to.
   *
   *  Only the LANGUAGE. Clock format and day-month order stay the phone's
   *  regional settings — see `localeFor` on the Swift side. */
  locale: string;
  strings: WidgetStrings;
  items: WidgetItem[];
}

/** Everything the builder needs, gathered by the caller — which is the only
 *  part that differs between platforms. */
export interface WidgetSnapshotInput<E extends RecurringEventLike> {
  /** Events as the backend returns them: masters, unexpanded. */
  events: E[];
  /** Which events mean the same appointment (DESIGN-event-groups.md).
   *
   *  Omitted, nothing folds — what the widget did before groups existed.
   *  Given, an appointment that lives in four calendars takes ONE line of the
   *  very little room a widget has, instead of all four. */
  eventGroups?: readonly EventGroup[];
  tasks: Task[];
  now: Date;
  /** How far ahead to look. */
  horizonDays: number;
  /** Hard cap on rows. */
  limit: number;
  /** The language the caller translated `strings` into. */
  locale: string;
  /** The non-data words, already translated by the caller. */
  strings: WidgetStrings;
  /** The connected user for a list's account, or null where the backend has no
   *  identity (a local list).
   *
   *  Drives the OWNERSHIP filter: a task assigned to a concrete other person and
   *  not to me is someone else's work, and the calendar views already leave it
   *  out. A widget that showed it would be quietly answering a different
   *  question — "what is on this shared board" instead of "what is next for
   *  you". Omitted ⇒ no ownership filtering at all. */
  meFor?: (listId: string) => TaskUser | null;
  /** Container ids the user has hidden on THIS device. Visibility is a
   *  per-device concern the core deliberately does not know about, so it is
   *  applied here, the same way the reminder scheduler applies it. */
  hiddenContainers?: ReadonlySet<string>;
  /** The event's RENDERED colour, `#rrggbb` or null. Per-item rather than
   *  per-container: an event can carry its own label or a provider's native
   *  colour, and the widget should show what the app shows. */
  eventColorOf?: (event: E) => string | null;
  /** The task's rendered colour, `#rrggbb` or null. */
  taskColorOf?: (task: Task) => string | null;
  /** An event's owning calendar id. Named rather than assumed, because the two
   *  frontends spell the field differently. */
  calendarIdOf: (event: E) => string;
  /** An event's title. Same reason. */
  titleOf: (event: E) => string;
  /** Whether an event is all-day. Same reason. */
  allDayOf: (event: E) => boolean;
}

const DAY_MS = 86_400_000;

/** Ceiling on how far before `now` the event expansion reaches back.
 *
 *  The lookback is normally the longest event's own duration, which is exact.
 *  This only guards the pathological input: one row with a corrupt or absurd
 *  span would otherwise widen the recurrence window for EVERY series, and a
 *  daily rule expanded across years is a lot of occurrences to build and throw
 *  away. A recurring event longer than a month that is "running right now" is
 *  not an answer to "what is next" anyway. */
const MAX_RUNNING_LOOKBACK_MS = 31 * 86_400_000;

/** How far back the expansion has to start so a series occurrence that is
 *  running right now is generated at all.
 *
 *  Every occurrence of a series carries the master's duration, so one still
 *  running began at most that long ago. Taking the longest across the whole set
 *  is therefore sufficient for all of them, and needs no per-series pass. */
function runningLookbackMs<E extends RecurringEventLike>(events: E[]): number {
  let longest = 0;
  for (const ev of events) {
    const span = new Date(ev.end).getTime() - new Date(ev.start).getTime();
    if (Number.isFinite(span) && span > longest) longest = span;
  }
  return Math.min(longest, MAX_RUNNING_LOOKBACK_MS);
}

/** Local midnight ENDING the day `at` falls in. */
function endOfDay(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate() + 1, 0, 0, 0, 0);
}

/** A timed event that has begun and not yet finished. */
function isRunning(item: WidgetItem, nowMs: number): boolean {
  return !item.untimed && item.end != null && new Date(item.at).getTime() <= nowMs;
}

/**
 * The instant a row is ordered by — which is not always the instant it starts.
 *
 * A timed item that has NOT started sorts at its start. Nothing surprising.
 *
 * One that is already RUNNING sorts at its end, because its start has stopped
 * being information: it is in the past, and the longer the thing has been going
 * the further ahead it sorts, which is backwards. What is still true about a
 * running event is when it STOPS.
 *
 * That is what makes nesting work. A calendar with "working hours, 10 to 16"
 * blocked out every day is an event like any other, and by start it beats every
 * real appointment inside it for six hours at a stretch — so the one widget
 * that shows a single row showed the block all morning and afternoon and never
 * the meeting actually happening. Ordered by end, the innermost thing comes
 * first: the 13:00 meeting ends before the block does, so it wins while it
 * runs, and the block returns the moment it is over.
 *
 * An UNTIMED item sorts at its END too, for the same reason one step further
 * on. Their starts are useless for ordering: an all-day event's start is
 * midnight, and a multi-day one started days or weeks ago. A fortnight's
 * holiday would answer "what is next" with "holiday" every day of the
 * fortnight — and on a lock screen with room for three rows, it and its
 * neighbours would crowd out every actual appointment.
 *
 * By END, an all-day event lands where it stops being true: a single day sits
 * with that day's appointments, a six-week holiday drops six weeks down the
 * list. The same reading puts an undated task due today after today's meetings
 * rather than in front of them, which is also where it belongs — a meeting has
 * an hour, the task has the day.
 */
function sortInstant(item: WidgetItem, nowMs: number): number {
  const at = new Date(item.at);
  if (!item.untimed) {
    return isRunning(item, nowMs) ? new Date(item.end as string).getTime() : at.getTime();
  }
  if (item.end != null) return new Date(item.end).getTime();
  return endOfDay(at).getTime();
}

/** Local midnight starting the day `key` (`YYYY-MM-DD`) names. */
function dayStart(key: string): Date {
  const [y, m, d] = key.split('-').map(Number);
  return new Date(y, (m ?? 1) - 1, d ?? 1, 0, 0, 0, 0);
}

/** `HH:MM[:SS]` on the local day `key`, as an instant. */
function atTimeOn(key: string, time: string): Date {
  const [h, min, s] = time.split(':').map(Number);
  const at = dayStart(key);
  at.setHours(h ?? 0, min ?? 0, s ?? 0, 0);
  return at;
}

/** Local day keys from today through the horizon, inclusive. */
function dayKeysThrough(now: Date, horizonDays: number): string[] {
  const keys: string[] = [];
  const cursor = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  for (let i = 0; i <= horizonDays; i += 1) {
    keys.push(localDateKey(cursor));
    cursor.setDate(cursor.getDate() + 1);
  }
  return keys;
}

/**
 * Build the snapshot the widget will render from.
 *
 * Selection rules, chosen to match what the app's own day view shows so the
 * widget never disagrees with the screen behind it:
 *   - an event is in if it has not ENDED yet (a meeting running right now is
 *     the most relevant row there is, and dropping it at its start time would
 *     be the one moment the user looks);
 *   - an all-day event is in for its whole day;
 *   - a task is in on the day `filterTasksOnDay` places it — which already
 *     drops cancelled, completed, and undated subtasks, and resolves the
 *     scheduled-vs-deadline question.
 */
/** Collapse each group to one row, a day at a time. See the call site for why
 *  the bucketing is not optional. */
function foldPerDay<E extends RecurringEventLike>(
  expanded: E[],
  groups: readonly EventGroup[],
  calendarIdOf: (event: E) => string,
  allDayOf: (event: E) => boolean,
): E[] {
  const byDay = new Map<string, E[]>();
  for (const ev of expanded) {
    const key = localDateKey(new Date(ev.start));
    byDay.set(key, [...(byDay.get(key) ?? []), ev]);
  }
  const kept: E[] = [];
  for (const dayEvents of byDay.values()) {
    const rows = collapseEventGroups(
      dayEvents.map((ev) => ({
        id: seriesIdOf(ev),
        calendar_id: calendarIdOf(ev),
        start: ev.start,
        all_day: allDayOf(ev),
        source: ev,
      })),
      groups,
      (row) => row.id,
    );
    for (const row of rows) kept.push(row.event.source);
  }
  return kept;
}

export function buildWidgetSnapshot<E extends RecurringEventLike>(
  input: WidgetSnapshotInput<E>,
): WidgetSnapshot {
  const {
    events,
    eventGroups,
    tasks,
    now,
    horizonDays,
    limit,
    locale,
    strings,
    meFor,
    hiddenContainers,
    eventColorOf,
    taskColorOf,
    calendarIdOf,
    titleOf,
    allDayOf,
  } = input;

  const horizonEnd = new Date(now.getTime() + horizonDays * DAY_MS);
  const nowMs = now.getTime();
  const hidden = hiddenContainers ?? new Set<string>();
  const items: WidgetItem[] = [];

  // ── Events ─────────────────────────────────────────────────────────────
  // Expanded over the window the widget will ask about, so a daily series
  // contributes each of its occurrences and not just its master — but starting
  // BEFORE `now`, and that part is load-bearing.
  //
  // `expandEvent` selects occurrences by their START. Expanding from `now`
  // therefore skips the occurrence that is running right now, because it began
  // earlier. A daily "working hours, 10 to 16" block was invisible all day
  // while TOMORROW's occurrence — which has not begun, so its start is inside
  // the window — came through and answered "what is next". The filter below
  // already keeps running events; it never got one to keep.
  //
  // How far back is not a guess. Every occurrence of a series carries the
  // master's duration, so anything still running began at most that long ago.
  const visibleEvents = events.filter((ev) => !hidden.has(calendarIdOf(ev)));
  const expandFrom = new Date(nowMs - runningLookbackMs(visibleEvents));
  // Folded PER DAY — the contract `collapseEventGroups` documents: across a
  // multi-day horizon a recurring appointment's own days look exactly like
  // copies that disagree, and then nothing folds at all. A widget has less
  // room than any view in the app, so a copy it need not show is a line it can
  // give to the next real thing.
  const expanded = expandAll(visibleEvents, { start: expandFrom, end: horizonEnd });
  const foldedEvents =
    eventGroups && eventGroups.length > 0
      ? foldPerDay(expanded, eventGroups, calendarIdOf, allDayOf)
      : expanded;
  for (const ev of foldedEvents) {
    // Over, not merely started. An all-day event needs no special case: its end
    // is the EXCLUSIVE next midnight, so this keeps it for the whole of its day
    // and drops it exactly when the day turns.
    if (new Date(ev.end).getTime() <= nowMs) continue;
    const containerId = calendarIdOf(ev);
    const color = eventColorOf?.(ev) ?? undefined;
    items.push({
      kind: 'event',
      id: ev.id,
      title: titleOf(ev),
      at: new Date(ev.start).toISOString(),
      end: new Date(ev.end).toISOString(),
      untimed: allDayOf(ev),
      containerId,
      ...(color ? { color } : {}),
    });
  }

  // ── Tasks ──────────────────────────────────────────────────────────────
  const dayKeys = dayKeysThrough(now, horizonDays);
  const firstKey = dayKeys[0] ?? localDateKey(now);
  const lastKey = dayKeys[dayKeys.length - 1] ?? firstKey;
  const visibleTasks = tasks.filter((t) => !hidden.has(t.list_id));
  // A recurring scheduled task is one row per occurrence over the window, the
  // same projection the calendar views render.
  const expandedTasks = expandScheduledRecurringTasks(visibleTasks, firstKey, lastKey);
  // An id can legitimately repeat across days (a deadline that also carries a
  // scheduled day is placed by filterTasksOnDay, but a projection walk can hand
  // the same task to two keys); one row per id keeps the widget honest.
  const seenTasks = new Set<string>();
  for (const key of dayKeys) {
    // `() => false` — completed tasks never belong on a "what is next" surface,
    // regardless of a list's show-completed setting. `meFor` applies the
    // ownership rule the calendar views apply.
    for (const task of filterTasksOnDay(expandedTasks, key, () => false, meFor)) {
      if (seenTasks.has(task.id)) continue;
      const time = taskTimeOnDay(task, key);
      const at = time ? atTimeOn(key, time) : dayStart(key);
      // A timed task whose time has passed is done being "next"; an untimed one
      // stands all day, because there is no moment at which it stopped being
      // today's business.
      if (time && at.getTime() <= nowMs) continue;
      seenTasks.add(task.id);
      const color = taskColorOf?.(task) ?? undefined;
      items.push({
        kind: 'task',
        id: task.id,
        title: task.title,
        at: at.toISOString(),
        untimed: time == null,
        containerId: task.list_id,
        ...(color ? { color } : {}),
        ...(task.status === 'in_progress'
          ? { status: 'in_progress' as const }
          : { status: 'open' as const }),
        ...(isRecurringProjection(task) ? {} : { completable: true }),
      });
    }
  }

  items.sort((a, b) => {
    const d = sortInstant(a, nowMs) - sortInstant(b, nowMs);
    if (d !== 0) return d;
    // Same instant, and one of them is already under way: that one. This is the
    // back-to-back case — a meeting ending at 14:00 and the next starting at
    // 14:00 both sort to 14:00 — where falling through to a title comparison
    // would put them in alphabetical order and call it a schedule.
    const aRunning = isRunning(a, nowMs);
    if (aRunning !== isRunning(b, nowMs)) return aRunning ? -1 : 1;
    // Then a timed row before an untimed one — it is the one with a commitment
    // attached — then events before tasks, then by title so the order is stable
    // across refreshes rather than incidental.
    if (a.untimed !== b.untimed) return a.untimed ? 1 : -1;
    if (a.kind !== b.kind) return a.kind === 'event' ? -1 : 1;
    return a.title.localeCompare(b.title);
  });

  return {
    version: WIDGET_SNAPSHOT_VERSION,
    generatedAt: now.toISOString(),
    horizonEnd: horizonEnd.toISOString(),
    locale,
    strings,
    items: items.slice(0, limit),
  };
}
