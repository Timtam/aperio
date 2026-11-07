/**
 * The iPhone's half of the Live Activities (DESIGN-live-activities.md, "iOS").
 *
 * The core decides WHAT an activity shows and when (`cal_core::live_activity`,
 * the plan). This decides what iOS can be told: it can start an activity only
 * while Aperio is in front, schedule one ahead only from iOS 26, change and
 * end one from the background too, and switch its content by itself exactly
 * once (`staleDate`). Swift only carries out the actions this returns.
 *
 * Pure, so the rules can be tested without a phone: what ActivityKit reports
 * and what Aperio remembers come in, the actions and the new memory go out.
 */

import type { LiveActivityChain, LiveActivitySection, LiveTriggerKey } from './types';

/** The words an activity says that are not data, in the app's language. */
export interface ActivityWords {
  /** Before a self-updating relative time: "Beginn" ("Beginn in 10 Minuten"). */
  begins: string;
  /** "Ende" ("Ende in 1 Stunde"). */
  ends: string;
  /** A template with `{title}` and `{time}`: "Danach: {title} um {time}". */
  followUp: string;
  /** What a finished appointment says once nothing follows: "Vorbei". */
  over: string;
}

/** One appointment as an activity shows it. Times are RFC 3339 instants. */
export interface ActivityShown {
  /** The appointment's key in the plan: what a swipe dismisses. */
  key: string;
  eventId: string;
  calendarId: string;
  start: string;
  end: string;
  title: string;
  location: string | null;
  followUpTitle: string | null;
  followUpStart: string | null;
}

/** An activity's content: what it shows now and, behind `staleAt`, next. */
export interface ActivityContent {
  /** BCP-47 tag of the language Aperio runs in (the region comes from the
   *  phone, `localeFor` in the widget extension). */
  locale: string;
  words: ActivityWords;
  current: ActivityShown;
  /** What iOS shows once `staleAt` has passed, without Aperio: the next
   *  appointment of the chain, or `null` to show `current` as over. */
  next: ActivityShown | null;
  staleAt: string;
  /** Higher wins the Dynamic Island: a later chain over an older one that
   *  only says it is over. */
  relevance: number;
}

/** An activity as ActivityKit reports it. */
export interface ReportedActivity {
  id: string;
  /** The chain key it was started under (its static attributes). */
  chainKey: string;
  state: 'pending' | 'active' | 'stale' | 'ended' | 'dismissed';
}

/** What Aperio remembers about an activity it started or scheduled. */
export interface RememberedActivity {
  /** Every appointment key the activity has shown or will show. */
  keys: string[];
  /** The appointment it showed last: what a swipe dismisses. */
  shownKey: string;
  /** When it started, or is scheduled to. */
  startsAt: string;
  /** The content last sent, so an unchanged one is not sent again: every
   *  update counts against the Apple Watch's budget. */
  content: string;
}

export interface ActivityMemory {
  activities: Record<string, RememberedActivity>;
  /** Appointment keys the user swiped away, with when, so they stay away. */
  dismissed: Record<string, string>;
}

export const EMPTY_ACTIVITY_MEMORY: ActivityMemory = { activities: {}, dismissed: {} };

export type ActivityAction =
  | { type: 'start'; chainKey: string; content: ActivityContent; keys: string[] }
  | {
      type: 'schedule';
      chainKey: string;
      content: ActivityContent;
      keys: string[];
      at: string;
      /** The reminders the scheduled start's alert stands for. */
      signal: LiveTriggerKey[];
    }
  | { type: 'update'; id: string; content: ActivityContent }
  | { type: 'end'; id: string };

export interface ReconcileInput {
  now: Date;
  chains: LiveActivityChain[];
  reported: ReportedActivity[];
  memory: ActivityMemory;
  /** Aperio is in front: only then may an activity start or be scheduled. */
  foreground: boolean;
  /** iOS 26 or later: an activity can be scheduled ahead. */
  canSchedule: boolean;
  /** Aperio's switch is on and iOS allows Live Activities. Off ends them all. */
  enabled: boolean;
  /** Whether the reminder a signal stands for is silent: a scheduled start
   *  always alerts, so a silent one is not scheduled. */
  isSilent: (signal: LiveTriggerKey[]) => boolean;
  locale: string;
  words: ActivityWords;
}

export interface ReconcileResult {
  actions: ActivityAction[];
  /** Reminders a waiting activity that still matches stands for: these, and
   *  only these, may be left out of the notifications. A schedule action adds
   *  its own once it has succeeded (`ActivityAction.signal`). */
  covered: LiveTriggerKey[];
  /** The memory for the activities that stay; `remember` adds a new one. */
  memory: ActivityMemory;
}

/** At most this many activities wait to start: scheduled ones count against
 *  the per-app limit (reported as five). */
export const MAX_PENDING = 2;

/** iOS ends an activity after eight hours; one gone before is a swipe. */
const SYSTEM_END_MS = 8 * 60 * 60 * 1000;

/** How long a swipe is remembered: an appointment is over well within it. */
const DISMISSAL_MEMORY_MS = 24 * 60 * 60 * 1000;

const ms = (iso: string) => Date.parse(iso);

function shown(section: LiveActivitySection): ActivityShown {
  return {
    key: section.event.key,
    eventId: section.event.item_id,
    calendarId: section.event.calendar_id,
    start: section.event.start,
    end: section.event.end,
    title: section.event.title,
    location: section.event.location,
    followUpTitle: section.follow_up?.title ?? null,
    followUpStart: section.follow_up?.start ?? null,
  };
}

/** The section on screen at `now`: the one running, else the first. */
function currentIndex(chain: LiveActivityChain, now: number): number {
  const i = chain.sections.findIndex((s) => ms(s.from) <= now && now < ms(s.until));
  return i < 0 ? 0 : i;
}

/**
 * What an activity for `chain` shows at `now`. The one switch iOS makes by
 * itself goes to the next appointment of the chain (the crossfade), and with
 * none to the end, where the same appointment reads as over (decision 10).
 */
export function activityContent(
  chain: LiveActivityChain,
  now: Date,
  locale: string,
  words: ActivityWords,
): ActivityContent {
  const i = currentIndex(chain, now.getTime());
  const current = chain.sections[i];
  const later = chain.sections.slice(i + 1).find((s) => s.event.key !== current.event.key);
  return {
    locale,
    words,
    current: shown(current),
    next: later ? shown(later) : null,
    staleAt: later ? later.from : chain.end,
    relevance: Math.floor(ms(chain.start) / 1000),
  };
}

/** The key a chain is started under: its first appointment. */
const chainKeyOf = (chain: LiveActivityChain) => chain.keys[0] ?? '';

/** The chain an activity belongs to: the one that shares any of its keys. */
function chainOf(
  chains: LiveActivityChain[],
  keys: readonly string[],
): LiveActivityChain | undefined {
  return chains.find((chain) => chain.keys.some((key) => keys.includes(key)));
}

/** The reminders the chain's first section stands for, if it is a signal. */
function firstSignal(chain: LiveActivityChain): LiveTriggerKey[] {
  const first = chain.sections[0];
  return first != null && first.from === chain.start ? first.signal : [];
}

/**
 * Compare what runs with what the plan says, and say what to do.
 *
 * Matching goes by appointment keys, never by the first one alone: once the
 * chain's first appointment has ended it drops out of the plan, and the
 * activity must still be found. A swipe can only be inferred — ActivityKit
 * tells no reason, and only while Aperio runs —, so an activity Aperio
 * started, did not end, and that is gone before iOS's eight hours counts as
 * swiped, and its appointment stays away (DESIGN, "Wegwischen").
 */
export function reconcileIosActivities(input: ReconcileInput): ReconcileResult {
  const now = input.now.getTime();
  const actions: ActivityAction[] = [];
  const covered: LiveTriggerKey[] = [];
  const activities: Record<string, RememberedActivity> = {};
  const dismissed: Record<string, string> = {};
  for (const [key, at] of Object.entries(input.memory.dismissed)) {
    if (now - ms(at) < DISMISSAL_MEMORY_MS) dismissed[key] = at;
  }
  const chains = input.chains.filter((chain) => ms(chain.end) > now);
  const reported = new Map(input.reported.map((a) => [a.id, a]));

  // Gone without Aperio ending it: a swipe, unless iOS's limit ran out.
  for (const [id, remembered] of Object.entries(input.memory.activities)) {
    const report = reported.get(id);
    const gone = report == null || report.state === 'dismissed' || report.state === 'ended';
    if (!gone) continue;
    const started = ms(remembered.startsAt);
    if (started <= now && now - started < SYSTEM_END_MS) {
      dismissed[remembered.shownKey] = input.now.toISOString();
    }
  }

  const served = new Set<LiveActivityChain>();
  let pending = 0;
  for (const report of input.reported) {
    if (report.state === 'ended' || report.state === 'dismissed') continue;
    // Aperio ends these itself, so they are no swipes next time: they leave
    // the memory here and are not counted as gone.
    if (!input.enabled) {
      actions.push({ type: 'end', id: report.id });
      continue;
    }
    const remembered = input.memory.activities[report.id];
    const chain = chainOf(chains, remembered?.keys ?? [report.chainKey]);
    if (chain == null || served.has(chain)) {
      actions.push({ type: 'end', id: report.id });
      continue;
    }
    if (report.state === 'pending') {
      // A start cannot be moved: one planned for another time, or for another
      // first appointment, is ended and planned again.
      const stillRight =
        remembered != null &&
        ms(remembered.startsAt) === ms(chain.start) &&
        report.chainKey === chainKeyOf(chain) &&
        pending < MAX_PENDING;
      if (!stillRight) {
        actions.push({ type: 'end', id: report.id });
        continue;
      }
      served.add(chain);
      pending += 1;
      covered.push(...firstSignal(chain));
      activities[report.id] = { ...remembered, keys: [...chain.keys] };
      continue;
    }
    served.add(chain);
    const content = activityContent(chain, input.now, input.locale, input.words);
    const sent = JSON.stringify(content);
    activities[report.id] = {
      keys: [...new Set([...(remembered?.keys ?? []), ...chain.keys])],
      shownKey: content.current.key,
      startsAt: remembered?.startsAt ?? chain.start,
      content: sent,
    };
    if (remembered?.content !== sent) actions.push({ type: 'update', id: report.id, content });
  }

  if (input.enabled && input.foreground) {
    for (const chain of chains) {
      if (served.has(chain)) continue;
      if (ms(chain.start) <= now) {
        // Running: started now, without an alert — the reminder rang already.
        const content = activityContent(chain, input.now, input.locale, input.words);
        if (dismissed[content.current.key] != null) continue;
        actions.push({ type: 'start', chainKey: chainKeyOf(chain), content, keys: [...chain.keys] });
        continue;
      }
      if (!input.canSchedule || pending >= MAX_PENDING) continue;
      const signal = firstSignal(chain);
      if (signal.length === 0 || input.isSilent(signal)) continue;
      pending += 1;
      actions.push({
        type: 'schedule',
        chainKey: chainKeyOf(chain),
        // As of its start: what the alert opens onto.
        content: activityContent(chain, new Date(ms(chain.start)), input.locale, input.words),
        keys: [...chain.keys],
        at: chain.start,
        signal,
      });
    }
  }

  return { actions, covered, memory: { activities, dismissed } };
}

/** The memory once a start or a schedule has given the activity its id. */
export function remember(
  memory: ActivityMemory,
  id: string,
  action: Extract<ActivityAction, { type: 'start' | 'schedule' }>,
  now: Date,
): ActivityMemory {
  return {
    ...memory,
    activities: {
      ...memory.activities,
      [id]: {
        keys: action.keys,
        shownKey: action.content.current.key,
        startsAt: action.type === 'schedule' ? action.at : now.toISOString(),
        content: JSON.stringify(action.content),
      },
    },
  };
}

/** A reminder's key with its instants as numbers: the plan and the
 *  notification list spell the same moment differently. */
export function triggerKeyId(key: {
  item_id: string;
  calendar_id: string;
  occurrence_start: string;
  fire_at: string;
}): string {
  return [key.item_id, key.calendar_id, ms(key.occurrence_start), ms(key.fire_at)].join('\n');
}
