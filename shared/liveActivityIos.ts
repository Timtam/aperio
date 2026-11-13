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
  /** The phone shows notification previews only when unlocked, or never:
   *  title, place and follow-up are then marked private on the lock screen. */
  hideOnLockScreen: boolean;
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
  /** The app build (`CFBundleVersion`) that wrote this memory. Absent in a
   *  memory written before it was kept, which then reads as another build. */
  build?: string;
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
  /** iOS 26 or later: an activity can be scheduled ahead. Decides which
   *  waiting activities are kept, even where nothing new may be planned. */
  canSchedule: boolean;
  /** New activities may be scheduled now: in front, with fresh data. The
   *  quick start from the last plan plans nothing. */
  planAhead: boolean;
  /** Aperio's switch is on and iOS allows Live Activities. Off ends them all. */
  enabled: boolean;
  /** Whether the reminder a signal stands for is silent: a scheduled start
   *  always alerts, so a silent one is not scheduled. */
  isSilent: (signal: LiveTriggerKey[]) => boolean;
  locale: string;
  words: ActivityWords;
  /** Notification previews are not shown on the locked phone. */
  hideOnLockScreen: boolean;
  /** The running app's build (`CFBundleVersion`). Another one than the
   *  memory's means the app was updated since: an activity gone then is no
   *  swipe. */
  build: string;
}

export interface ReconcileResult {
  actions: ActivityAction[];
  /** Reminders a waiting activity that still matches stands for: these, and
   *  only these, may be left out of the notifications. A schedule action adds
   *  its own once it has succeeded (`ActivityAction.signal`). */
  covered: LiveTriggerKey[];
  /** The memory for the activities that stay; `remember` adds a new one. */
  memory: ActivityMemory;
  /** The earliest end among the chains that keep or get an activity, or
   *  `null` with none. Only a running Aperio can end an activity, and iOS
   *  wakes it when it likes; this is the moment to ask it for (TODO A12,
   *  Toni's iPhone 2026-11-10: "Vorbei" on the lock screen all evening). */
  wakeAt: string | null;
}

/** At most this many activities wait to start, the earliest ones: scheduled
 *  ones count against the per-app limit (reported as five). */
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
  hideOnLockScreen = false,
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
    hideOnLockScreen,
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
 * swiped, and its appointment stays away (DESIGN, "Wegwischen"). Not after an
 * update of the app: the running activity vanished then, read as a swipe,
 * and stayed away (Toni's iPhone, build 67, 2026-11-08).
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

  // Gone without Aperio ending it: a swipe, unless iOS's limit ran out — or
  // Live Activities are off, when iOS removes them all itself, or the app was
  // updated since the memory was written.
  const updated = (input.memory.build ?? '') !== input.build;
  if (input.enabled && !updated) {
    for (const [id, remembered] of Object.entries(input.memory.activities)) {
      const report = reported.get(id);
      const gone = report == null || report.state === 'dismissed' || report.state === 'ended';
      if (!gone) continue;
      const started = ms(remembered.startsAt);
      if (started <= now && now - started < SYSTEM_END_MS) {
        dismissed[remembered.shownKey] = input.now.toISOString();
      }
    }
  }

  // The chains that should wait to start: the EARLIEST few, never a silent
  // reminder's (a scheduled start always alerts).
  const toSchedule = new Set(
    input.canSchedule
      ? chains
          .filter((chain) => ms(chain.start) > now)
          .filter((chain) => {
            const signal = firstSignal(chain);
            return signal.length > 0 && !input.isSilent(signal);
          })
          .slice(0, MAX_PENDING)
      : [],
  );
  const contentAtStart = (chain: LiveActivityChain) =>
    activityContent(chain, new Date(ms(chain.start)), input.locale, input.words, input.hideOnLockScreen);

  const served = new Set<LiveActivityChain>();
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
      // A start cannot be moved: one planned for another time, for another
      // first appointment, for a reminder now silent, or beyond the earliest
      // few, is ended — and planned again in front.
      const sent = JSON.stringify(contentAtStart(chain));
      const stillRight =
        remembered != null &&
        ms(remembered.startsAt) === ms(chain.start) &&
        report.chainKey === chainKeyOf(chain) &&
        toSchedule.has(chain);
      // What it will show has changed (a new title, room, follow-up): planned
      // again where that is possible; from the background it stays as it is.
      const stale = remembered != null && remembered.content !== sent;
      if (!stillRight || (stale && input.foreground && input.planAhead)) {
        actions.push({ type: 'end', id: report.id });
        continue;
      }
      served.add(chain);
      covered.push(...firstSignal(chain));
      activities[report.id] = { ...remembered, keys: [...chain.keys] };
      continue;
    }
    // A running activity never jumps ahead to a chain that has not begun: an
    // appointment that has left its chain must not pull the activity along.
    if (ms(chain.start) > now && !(remembered != null && chain.keys.includes(remembered.shownKey))) {
      actions.push({ type: 'end', id: report.id });
      continue;
    }
    served.add(chain);
    const content = activityContent(chain, input.now, input.locale, input.words, input.hideOnLockScreen);
    const sent = JSON.stringify(content);
    activities[report.id] = {
      // The chain's keys as they are now: they hold the next appointment
      // already, which is what finds the activity once the first is over.
      keys: [...chain.keys],
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
        const content = activityContent(chain, input.now, input.locale, input.words, input.hideOnLockScreen);
        if (dismissed[content.current.key] != null) continue;
        actions.push({ type: 'start', chainKey: chainKeyOf(chain), content, keys: [...chain.keys] });
        served.add(chain);
        continue;
      }
      if (!input.planAhead || !toSchedule.has(chain)) continue;
      served.add(chain);
      actions.push({
        type: 'schedule',
        chainKey: chainKeyOf(chain),
        // As of its start: what the alert opens onto.
        content: contentAtStart(chain),
        keys: [...chain.keys],
        at: chain.start,
        signal: firstSignal(chain),
      });
    }
  }

  let wakeAt: string | null = null;
  for (const chain of served) {
    if (wakeAt == null || ms(chain.end) < ms(wakeAt)) wakeAt = chain.end;
  }

  return { actions, covered, memory: { activities, dismissed, build: input.build }, wakeAt };
}

/** Where a pass ran: in front, in an OS background round, or the quick start
 *  from the last plan on a return to the app. */
export type ActivityPassOrigin = 'foreground' | 'background' | 'quick start';

/**
 * One pass as one log line, or `null` when there was nothing to say: no
 * activity before, none touched. Counts only, no titles and no ids.
 *
 * The background round is the one path nobody watches, and an activity left
 * on the lock screen after its appointment could mean either that no round
 * ran or that a round ran and kept it. Without this line the two looked the
 * same (Toni's iPhone, 2026-11-10).
 */
export function activityPassSummary(pass: {
  origin: ActivityPassOrigin;
  /** Activities running or waiting when the pass began. */
  before: number;
  /** The actions iOS accepted, by type. */
  done: ActivityAction['type'][];
  /** The actions iOS refused (each logged on its own). */
  failed: number;
  wakeAt: string | null;
}): string | null {
  if (pass.before === 0 && pass.done.length === 0 && pass.failed === 0) return null;
  const count = (type: ActivityAction['type']) => pass.done.filter((t) => t === type).length;
  return (
    `live activities (${pass.origin}): before=${pass.before}` +
    ` ended=${count('end')} updated=${count('update')}` +
    ` started=${count('start')} scheduled=${count('schedule')}` +
    ` failed=${pass.failed} wake=${pass.wakeAt ?? 'none'}`
  );
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
