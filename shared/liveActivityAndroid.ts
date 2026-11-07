/**
 * The Android half of the Live Activities (DESIGN-live-activities.md,
 * "Android"): the plan, flattened into what the native notifier works from.
 *
 * Android needs no reconciling: an exact alarm wakes Aperio at every section
 * boundary, so the notifier simply shows the section that runs. What it needs
 * is the plan in a form it can read without the core and without JavaScript —
 * instants as milliseconds, each section with the channel it rings on and, if
 * it IS a reminder, a stable signal id the notifier remembers once it rang.
 *
 * Pure, so the shape is tested where the rest of the rules are.
 */

import { triggerKeyId } from './liveActivityIos';
import type { LiveActivityPlan, LiveTriggerKey } from './types';

/** The sentences' templates, in the language Aperio runs in. `{time}` and
 *  `{title}` are filled in by the notifier with the phone's own time format. */
export interface AndroidTexts {
  /** "Beginnt um {time}". */
  beginsAt: string;
  /** "Läuft bis {time}". */
  runsUntil: string;
  /** "Danach: {title} um {time}". */
  followUp: string;
  /** Between the parts of the sentence: " · ". */
  separator: string;
}

/** One stretch of time over which the notification does not change. */
export interface AndroidSection {
  from: number;
  until: number;
  /** The appointment's key: what a swipe dismisses. */
  key: string;
  eventId: string;
  calendarId: string;
  title: string;
  location: string | null;
  start: number;
  end: number;
  followUpTitle: string | null;
  followUpStart: number | null;
  /** The appointment has begun: "Läuft bis" and a countdown to the end. */
  started: boolean;
  /** May ask Android to promote it (a Live Update). */
  promoted: boolean;
  /** The channel it shows on: the appointment's reminder channel, so it
   *  rings with that reminder's sound when it IS the reminder. */
  channelId: string;
  /** Set when this section IS a reminder: a NEW notification that rings and is
   *  announced. Every other section changes it silently. */
  signal: string | null;
  /** The end of its chain, after which the notification goes. */
  chainEnd: number;
}

export interface AndroidHandover {
  version: 1;
  texts: AndroidTexts;
  /** Time order; sections never overlap. */
  sections: AndroidSection[];
}

const ms = (iso: string) => Date.parse(iso);

/**
 * Flatten the plan for the notifier. `channelOf` gives the channel of a
 * reminder by its trigger key id (`triggerKeyId`); a section that is no
 * reminder takes its appointment's reminder channel from the chain, else
 * `defaultChannel`.
 */
export function androidHandover(
  plan: LiveActivityPlan,
  texts: AndroidTexts,
  channelOf: (triggerKeyIdOfReminder: string) => string | undefined,
  defaultChannel: string,
): AndroidHandover {
  const sections: AndroidSection[] = [];
  for (const chain of plan.chains) {
    const signalChannel = (signal: LiveTriggerKey[]) =>
      signal.map((key) => channelOf(triggerKeyId(key))).find((c) => c != null);
    // Each appointment rings on its own reminder's channel, also in the
    // sections that are not that reminder.
    const channelByKey = new Map<string, string>();
    for (const section of chain.sections) {
      const channel = signalChannel(section.signal);
      if (channel != null && !channelByKey.has(section.event.key)) {
        channelByKey.set(section.event.key, channel);
      }
    }
    for (const section of chain.sections) {
      const from = ms(section.from);
      sections.push({
        from,
        until: ms(section.until),
        key: section.event.key,
        eventId: section.event.item_id,
        calendarId: section.event.calendar_id,
        title: section.event.title,
        location: section.event.location,
        start: ms(section.event.start),
        end: ms(section.event.end),
        followUpTitle: section.follow_up?.title ?? null,
        followUpStart: section.follow_up ? ms(section.follow_up.start) : null,
        started: section.started,
        promoted: section.promoted,
        channelId:
          signalChannel(section.signal) ?? channelByKey.get(section.event.key) ?? defaultChannel,
        signal: section.signal.length > 0 ? `${section.event.key}@${from}` : null,
        chainEnd: ms(chain.end),
      });
    }
  }
  sections.sort((a, b) => a.from - b.from);
  return { version: 1, texts, sections };
}
