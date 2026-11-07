// The Android Live Activities (DESIGN-live-activities.md, "Android"): the plan
// is handed to the native notifier (LiveActivityNotifier.kt), which from then
// on shows the ongoing notification and arms its alarms itself. Here, inside
// the reminder pass, only the hand-over happens — and the decision which
// reminders Expo leaves out: the ones the plan absorbed, and only once the
// notifier has taken the plan. Any failure stops the notifier and gives every
// reminder back to Expo, so nothing rings twice.

import AsyncStorage from '@react-native-async-storage/async-storage';
import {
  androidHandover,
  rangByExpo,
  reminderPairId,
  type AndroidTexts,
} from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import i18n from '../../i18n';
import { liveActivityPlan } from '../api/liveActivities';
import { logLine } from '../api/logs';
import { upcomingReminders, type UpcomingReminder } from '../api/reminders';
import { readLiveActivitiesEnabled } from '../state/liveActivitiesPref';
import { reminderKeyId } from './reminderKey';

/**
 * Every reminder some hand-over gave the notifier, with when it was due: Expo
 * left those out, so a past one the notifier holds is its own to ring. Every
 * other past reminder of the plan Expo rang (`rangByExpo`). Cleared whenever
 * the notifier stops, so the next hand-over counts all the past as Expo's.
 */
const OWNED_KEY = 'aperio.liveActivities.android.owned.v1';
/** A reminder is forgotten a day after it was due: its appointment is over. */
const OWNED_MEMORY_MS = 24 * 60 * 60 * 1000;

function texts(): AndroidTexts {
  return {
    beginsAt: i18n.t('widgets.liveActivity.beginsAt'),
    runsUntil: i18n.t('widgets.liveActivity.runsUntil'),
    followUp: i18n.t('widgets.liveActivity.followUp'),
    separator: ' · ',
  };
}

async function readOwned(now: number): Promise<Map<string, number>> {
  try {
    const raw = await AsyncStorage.getItem(OWNED_KEY);
    const stored = raw == null ? {} : (JSON.parse(raw) as Record<string, number>);
    return new Map(Object.entries(stored).filter(([, due]) => now - due < OWNED_MEMORY_MS));
  } catch {
    return new Map();
  }
}

/** Stop the notifier and give every reminder back to Expo. */
async function stop(horizonMinutes: number) {
  await CalFfi.liveActivityClear().catch(() => {});
  await AsyncStorage.removeItem(OWNED_KEY).catch(() => {});
  return { items: await upcomingReminders(horizonMinutes), covered: new Set<string>() };
}

export async function androidRemindersAndActivities(
  horizonMinutes: number,
  hidden: ReadonlySet<string>,
  channels: { channelFor: (r: UpcomingReminder) => Promise<string>; defaultChannel: string },
): Promise<{ items: UpcomingReminder[]; covered: Set<string> }> {
  // Switched off: nothing shows, every reminder is Expo's again.
  if (!(await readLiveActivitiesEnabled())) return stop(horizonMinutes);
  let answer: Awaited<ReturnType<typeof liveActivityPlan>>;
  try {
    answer = await liveActivityPlan(horizonMinutes, [...hidden]);
  } catch (err) {
    void logLine(
      'warn',
      `live activity plan failed, reminders only: ${err instanceof Error ? err.message : String(err)}`,
    );
    return stop(horizonMinutes);
  }
  try {
    const now = Date.now();
    // The channel each absorbed reminder would have rung on: the notification
    // that IS the reminder rings on it, with its sound.
    const byPair = new Map<string, string>();
    for (const r of answer.reminders) {
      if (r.absorbed) byPair.set(reminderPairId(r.item_id, r.trigger_at), await channels.channelFor(r));
    }
    const handover = androidHandover(answer.plan, i18n.language, texts(), (id) => byPair.get(id));
    const owned = await readOwned(now);
    const rang = rangByExpo(handover, new Set(owned.keys()), now);
    await CalFfi.liveActivityHandOver(JSON.stringify(handover), JSON.stringify(rang));
    // From now on the notifier rings these; Expo leaves them out.
    for (const section of handover.sections) {
      for (const id of section.signal ?? []) {
        const due = Number(id.slice(id.lastIndexOf('\n') + 1));
        if (due > now) owned.set(id, due);
      }
    }
    await AsyncStorage.setItem(OWNED_KEY, JSON.stringify(Object.fromEntries(owned))).catch(() => {});
    const covered = new Set(answer.reminders.filter((r) => r.absorbed).map(reminderKeyId));
    return { items: answer.reminders, covered };
  } catch (err) {
    void logLine(
      'warn',
      `live activities not handed over, reminders back with Expo: ${err instanceof Error ? err.message : String(err)}`,
    );
    return stop(horizonMinutes);
  }
}
