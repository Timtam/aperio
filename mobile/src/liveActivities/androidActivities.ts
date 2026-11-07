// The Android Live Activities (DESIGN-live-activities.md, "Android"): the plan
// is handed to the native notifier (LiveActivityNotifier.kt), which from then
// on shows the ongoing notification and arms its alarms itself. Here, inside
// the reminder pass, only the hand-over happens — and the decision which
// reminders Expo leaves out: the ones the plan absorbed, and only once the
// notifier has taken the plan. Any failure keeps every reminder Expo's.

import AsyncStorage from '@react-native-async-storage/async-storage';
import { androidHandover, type AndroidTexts } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import i18n from '../../i18n';
import { liveActivityPlan } from '../api/liveActivities';
import { logLine } from '../api/logs';
import { upcomingReminders, type UpcomingReminder } from '../api/reminders';
import { readLiveActivitiesEnabled } from '../state/liveActivitiesPref';
import { reminderKeyId } from './reminderKey';

/** Set once the notifier holds a plan; absent means Expo rang every reminder
 *  so far, which the next hand-over must know (`freshStart`). */
const HANDED_OVER_KEY = 'aperio.liveActivities.android.handedOver';

function texts(): AndroidTexts {
  return {
    beginsAt: i18n.t('widgets.liveActivity.beginsAt'),
    runsUntil: i18n.t('widgets.liveActivity.runsUntil'),
    followUp: i18n.t('widgets.liveActivity.followUp'),
    separator: ' · ',
  };
}

async function plain(horizonMinutes: number) {
  return { items: await upcomingReminders(horizonMinutes), covered: new Set<string>() };
}

export async function androidRemindersAndActivities(
  horizonMinutes: number,
  hidden: ReadonlySet<string>,
  channels: { channelFor: (r: UpcomingReminder) => Promise<string>; defaultChannel: string },
): Promise<{ items: UpcomingReminder[]; covered: Set<string> }> {
  if (!(await readLiveActivitiesEnabled())) {
    // Switched off: nothing shows, every reminder is Expo's again.
    await CalFfi.liveActivityClear().catch(() => {});
    await AsyncStorage.removeItem(HANDED_OVER_KEY).catch(() => {});
    return plain(horizonMinutes);
  }
  let answer: Awaited<ReturnType<typeof liveActivityPlan>>;
  try {
    answer = await liveActivityPlan(horizonMinutes, [...hidden]);
  } catch (err) {
    void logLine(
      'warn',
      `live activity plan failed, reminders only: ${err instanceof Error ? err.message : String(err)}`,
    );
    return plain(horizonMinutes);
  }
  try {
    // The channel each absorbed reminder would have rung on: the notification
    // that IS the reminder rings on it, with its sound.
    const byKey = new Map<string, string>();
    for (const r of answer.reminders) {
      if (r.absorbed) byKey.set(reminderKeyId(r), await channels.channelFor(r));
    }
    const handover = androidHandover(
      answer.plan,
      texts(),
      (id) => byKey.get(id),
      channels.defaultChannel,
    );
    const freshStart = (await AsyncStorage.getItem(HANDED_OVER_KEY).catch(() => null)) == null;
    await CalFfi.liveActivityHandOver(JSON.stringify(handover), freshStart);
    await AsyncStorage.setItem(HANDED_OVER_KEY, '1').catch(() => {});
    const covered = new Set(answer.reminders.filter((r) => r.absorbed).map(reminderKeyId));
    return { items: answer.reminders, covered };
  } catch (err) {
    void logLine(
      'warn',
      `live activities not handed over, reminders stay with Expo: ${err instanceof Error ? err.message : String(err)}`,
    );
    return { items: answer.reminders, covered: new Set() };
  }
}
