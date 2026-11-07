/**
 * The Android half of the Live Activities (DESIGN-live-activities.md,
 * "Android"): the plan, flattened into what the native notifier works from.
 *
 * Android needs no reconciling: an exact alarm wakes Aperio at every section
 * boundary, so the notifier simply shows the section that runs. What it needs
 * is the plan in a form it can read without the core and without JavaScript —
 * instants as milliseconds, each section with the channel it rings on and, if
 * it IS a reminder, which reminders, so the notifier can remember they rang.
 *
 * Pure, so the shape is tested where the rest of the rules are.
 */

import type { LiveActivityPlan } from './types';

/** The sentences' templates, in the language Aperio runs in. `{time}` and
 *  `{title}` are filled in by the notifier. */
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
  /** The channel it shows on: the appointment's reminder channel, so it rings
   *  with that reminder's sound when it IS the reminder. `null` when not known
   *  now — a reminder already past is no longer in the notification list — and
   *  the notifier keeps the one it knew. */
  channelId: string | null;
  /** Set when this section IS a reminder: the reminders it stands for, as
   *  `reminderPairId`s. A NEW notification that rings and is announced, unless
   *  one of them rang already. Every other section changes it silently. */
  signal: string[] | null;
  /** The end of its chain, after which the notification goes; also what
   *  tells the chains apart. */
  chainEnd: number;
}

export interface AndroidHandover {
  version: 2;
  /** BCP-47 tag of Aperio's language, for the clock times. */
  language: string;
  texts: AndroidTexts;
  /** Time order; sections never overlap. */
  sections: AndroidSection[];
}

const ms = (iso: string) => Date.parse(iso);

/**
 * A reminder as the notifier remembers it: its item and its moment — the same
 * pair the notification list keeps one entry for, so the twin of a reminder
 * that rang counts as rung too. Instants as numbers: the plan and the list
 * spell the same moment differently.
 */
export function reminderPairId(itemId: string, fireAt: string): string {
  return `${itemId}\n${ms(fireAt)}`;
}

/**
 * Flatten the plan for the notifier. `channelOf` gives the channel of a
 * reminder by its `reminderPairId`; a section that is no reminder takes its
 * appointment's reminder channel from the chain, else `null`.
 */
export function androidHandover(
  plan: LiveActivityPlan,
  language: string,
  texts: AndroidTexts,
  channelOf: (pairId: string) => string | undefined,
): AndroidHandover {
  const sections: AndroidSection[] = [];
  for (const chain of plan.chains) {
    const pairs = (section: (typeof chain.sections)[number]) =>
      section.signal.map((key) => reminderPairId(key.item_id, key.fire_at));
    const signalChannel = (ids: string[]) =>
      ids.map((id) => channelOf(id)).find((c) => c != null);
    // Each appointment rings on its own reminder's channel, also in the
    // sections that are not that reminder.
    const channelByKey = new Map<string, string>();
    for (const section of chain.sections) {
      const channel = signalChannel(pairs(section));
      if (channel != null && !channelByKey.has(section.event.key)) {
        channelByKey.set(section.event.key, channel);
      }
    }
    for (const section of chain.sections) {
      const ids = pairs(section);
      sections.push({
        from: ms(section.from),
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
        channelId: signalChannel(ids) ?? channelByKey.get(section.event.key) ?? null,
        signal: ids.length > 0 ? [...ids].sort() : null,
        chainEnd: ms(chain.end),
      });
    }
  }
  sections.sort((a, b) => a.from - b.from);
  return { version: 2, language, texts, sections };
}

/**
 * The reminders the notifier must treat as rung although it never rang them:
 * those of a section already begun that Expo still held at the last
 * hand-over — Expo rang them. `ownedByNotifier` is every reminder some
 * hand-over gave the notifier; with none (the first hand-over, or one after
 * the switch was turned on) every past reminder was Expo's.
 */
export function rangByExpo(
  handover: AndroidHandover,
  ownedByNotifier: ReadonlySet<string>,
  now: number,
): string[] {
  const out = new Set<string>();
  for (const section of handover.sections) {
    if (section.signal == null) continue;
    for (const id of section.signal) {
      const fireAt = Number(id.slice(id.lastIndexOf('\n') + 1));
      if (fireAt <= now && !ownedByNotifier.has(id)) out.add(id);
    }
  }
  return [...out].sort();
}
