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
import { expandAll, type RecurringEventLike } from './recurrence';
import { filterTasksOnDay, taskTimeOnDay } from './taskDay';
import type { Task } from './types';

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
   *  Spoken at the END of a row, after the identifying content. */
  kindEvent: string;
  kindTask: string;
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

/** Local midnight ENDING the day `at` falls in. */
function endOfDay(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate() + 1, 0, 0, 0, 0);
}

/**
 * The instant a row is ordered by — which is not always the instant it starts.
 *
 * A timed item sorts at its start, including one that is already running: a
 * meeting in progress is the most immediate thing there is.
 *
 * An UNTIMED item sorts at its END instead, and that is the whole point. Their
 * starts are useless for ordering: an all-day event's start is midnight, and a
 * multi-day one started days or weeks ago, so by start they all sort ahead of
 * everything. A fortnight's holiday would then answer "what is next" with
 * "holiday" every day of the fortnight — and on a lock screen with room for
 * three rows, it and its neighbours would crowd out every actual appointment.
 *
 * By END, an all-day event lands where it stops being true: a single day sits
 * with that day's appointments, a six-week holiday drops six weeks down the
 * list. The same reading puts an undated task due today after today's meetings
 * rather than in front of them, which is also where it belongs — a meeting has
 * an hour, the task has the day.
 */
function sortInstant(item: WidgetItem): number {
  const at = new Date(item.at);
  if (!item.untimed) return at.getTime();
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
export function buildWidgetSnapshot<E extends RecurringEventLike>(
  input: WidgetSnapshotInput<E>,
): WidgetSnapshot {
  const {
    events,
    tasks,
    now,
    horizonDays,
    limit,
    locale,
    strings,
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
  // Expanded over the same window the widget will ask about, so a daily series
  // contributes each of its occurrences and not just its master.
  const visibleEvents = events.filter((ev) => !hidden.has(calendarIdOf(ev)));
  for (const ev of expandAll(visibleEvents, { start: now, end: horizonEnd })) {
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
    // regardless of a list's show-completed setting.
    for (const task of filterTasksOnDay(expandedTasks, key, () => false)) {
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
        ...(isRecurringProjection(task) ? {} : { completable: true }),
      });
    }
  }

  items.sort((a, b) => {
    const d = sortInstant(a) - sortInstant(b);
    if (d !== 0) return d;
    // Same instant: a timed row first — it is the one with a commitment attached
    // — then events before tasks, then by title so the order is stable across
    // refreshes rather than incidental.
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
