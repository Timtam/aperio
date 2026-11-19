// The iPhone's Live Activities, carried out (DESIGN-live-activities.md, "iOS").
//
// `shared/liveActivityIos.ts` decides; this asks ActivityKit what runs, hands
// the decision the plan, performs the actions through the native module and
// remembers what it started. It runs inside the reminder pass, so the plan and
// the notification list come from the same scan, and it says which reminders
// a waiting activity stands for: those, and only those, are left out of the
// notifications (one signal, decision 9).

import AsyncStorage from '@react-native-async-storage/async-storage';
import {
  EMPTY_ACTIVITY_MEMORY,
  activityEnds,
  activityPassSummary,
  nextWake,
  reconcileIosActivities,
  remember,
  triggerKeyId,
  type ActivityAction,
  type ActivityMemory,
  type ActivityPassOrigin,
  type ActivityWords,
  type LiveActivityPlan,
  type LiveTriggerKey,
  type ReportedActivity,
} from '@aperio/shared';
import { AppState, Platform } from 'react-native';

import CalFfi from '../../modules/cal-ffi';
import i18n from '../../i18n';
import { liveActivityPlan, type PlannedReminder } from '../api/liveActivities';
import { logLine } from '../api/logs';
import { upcomingReminders, type UpcomingReminder } from '../api/reminders';
import { readLiveActivitiesEnabled } from '../state/liveActivitiesPref';
import { reminderKeyId } from './reminderKey';

const MEMORY_KEY = 'aperio.liveActivities.ios.memory.v1';
const PLAN_KEY = 'aperio.liveActivities.ios.plan.v1';

interface Support {
  available: boolean;
  enabled: boolean;
  canSchedule: boolean;
  previews: 'always' | 'whenUnlocked' | 'never';
  /** The app's build (`CFBundleVersion`); absent from a module before it. */
  build?: string;
  /** Activities the native wake ended at their end, of the last day; absent
   *  from a module before it. */
  endedByWake?: string[];
}


function words(): ActivityWords {
  return {
    begins: i18n.t('widgets.liveActivity.begins'),
    ends: i18n.t('widgets.liveActivity.ends'),
    followUp: i18n.t('widgets.liveActivity.followUp'),
    over: i18n.t('widgets.liveActivity.over'),
  };
}

async function readMemory(): Promise<ActivityMemory> {
  try {
    const raw = await AsyncStorage.getItem(MEMORY_KEY);
    return raw == null ? EMPTY_ACTIVITY_MEMORY : (JSON.parse(raw) as ActivityMemory);
  } catch {
    return EMPTY_ACTIVITY_MEMORY;
  }
}

async function writeMemory(memory: ActivityMemory): Promise<void> {
  await AsyncStorage.setItem(MEMORY_KEY, JSON.stringify(memory)).catch(() => {});
}

/** One run at a time: the quick start on return and the full pass both
 *  start activities, and two at once would start the same one twice. */
let running: Promise<unknown> = Promise.resolve();
function serially<T>(run: () => Promise<T>): Promise<T> {
  const next = running.then(run, run);
  running = next.catch(() => {});
  return next;
}

/** Ask ActivityKit, decide, act. Returns the reminders a waiting activity
 *  stands for, as trigger key ids. */
function syncIosActivities(
  plan: LiveActivityPlan,
  reminders: PlannedReminder[],
  opts: { foreground: boolean; mayPlan: boolean; origin: ActivityPassOrigin },
): Promise<Set<string>> {
  return serially(async () => {
    const covered = new Set<string>();
    const support = JSON.parse(await CalFfi.liveActivitySupportJson()) as Support;
    if (!support.available) return covered;
    const enabled = support.enabled && (await readLiveActivitiesEnabled());
    const reported = JSON.parse(await CalFfi.liveActivityListJson()) as ReportedActivity[];
    const now = new Date();
    const silent = new Set(
      reminders.filter((r) => r.sound.source.type === 'silent').map(reminderKeyId),
    );
    const byKey = new Map(reminders.map((r) => [reminderKeyId(r), r]));
    const result = reconcileIosActivities({
      now,
      chains: plan.chains,
      reported,
      memory: await readMemory(),
      foreground: opts.foreground,
      canSchedule: support.canSchedule,
      planAhead: opts.mayPlan,
      enabled,
      isSilent: (signal) => signal.some((k) => silent.has(triggerKeyId(k))),
      locale: i18n.language,
      words: words(),
      hideOnLockScreen: support.previews !== 'always',
      build: support.build ?? '',
      endedByWake: support.endedByWake ?? [],
    });
    for (const key of result.covered) covered.add(triggerKeyId(key));
    let memory = result.memory;
    const done: ActivityAction['type'][] = [];
    let failed = 0;
    for (const action of result.actions) {
      try {
        switch (action.type) {
          case 'end':
            await CalFfi.liveActivityEnd(action.id);
            break;
          case 'update':
            await CalFfi.liveActivityUpdate(action.id, JSON.stringify(action.content));
            break;
          case 'start': {
            const id = await CalFfi.liveActivityStart(action.chainKey, JSON.stringify(action.content));
            memory = remember(memory, id, action, now);
            break;
          }
          case 'schedule': {
            const alert = alertFor(action.signal, byKey);
            const id = await CalFfi.liveActivitySchedule(
              action.chainKey,
              JSON.stringify(action.content),
              action.at,
              alert.title,
              alert.body,
            );
            memory = remember(memory, id, action, now);
            // Only now does the activity stand for the reminder.
            for (const key of action.signal) covered.add(triggerKeyId(key));
            break;
          }
        }
        done.push(action.type);
      } catch (err) {
        failed += 1;
        // iOS refused (the limit, the switch, an old system). The reminder
        // then rings as a notification, which is the safe side.
        void logLine(
          'warn',
          `live activity ${action.type} failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }
    // Only a running Aperio can end an activity, and iOS wakes it when it
    // likes: hand each activity's end to the native side, which asks for the
    // next background round at the earliest one and, when woken, ends the
    // overdue ones before the round's JS runs. A wish iOS may defer, never a
    // schedule. Handed over BEFORE the memory is written: cut off between the
    // two, the native side keeps an older end and ends the activity at it,
    // which it reports (`endedByWake`), so nothing reads that as a swipe. The
    // other order could leave a memory that knows nothing of such an end.
    const ends = activityEnds(memory);
    await CalFfi.liveActivityEnds(JSON.stringify(ends)).catch(() => {});
    await writeMemory(memory);
    const summary = activityPassSummary({
      origin: opts.origin,
      before: reported.filter((a) => a.state !== 'ended' && a.state !== 'dismissed').length,
      done,
      failed,
      wakeAt: nextWake(ends, now),
    });
    if (summary != null) void logLine('info', summary);
    return covered;
  });
}

/** The scheduled start's alert: what the reminder would have said. Shown on
 *  the Apple Watch; on the iPhone the activity itself is the signal. */
function alertFor(
  signal: LiveTriggerKey[],
  byKey: Map<string, PlannedReminder>,
): { title: string; body: string } {
  for (const key of signal) {
    const r = byKey.get(triggerKeyId(key));
    if (r) return { title: r.title, body: r.body };
  }
  return { title: '', body: '' };
}

/**
 * The notification list, and on the iPhone the Live Activities kept in step
 * with it. `covered` holds the reminders a waiting activity stands for. On
 * any failure the list comes from the plain scan and nothing is covered: then
 * every reminder rings as before.
 */
export async function iosRemindersAndActivities(
  horizonMinutes: number,
  hidden: ReadonlySet<string>,
): Promise<{ items: UpcomingReminder[]; covered: Set<string> }> {
  let answer: Awaited<ReturnType<typeof liveActivityPlan>>;
  try {
    answer = await liveActivityPlan(horizonMinutes, [...hidden]);
  } catch (err) {
    void logLine(
      'warn',
      `live activity plan failed, reminders only: ${err instanceof Error ? err.message : String(err)}`,
    );
    return { items: await upcomingReminders(horizonMinutes), covered: new Set() };
  }
  let covered = new Set<string>();
  try {
    // Kept for the next return to the app, which starts from it at once.
    await AsyncStorage.setItem(PLAN_KEY, JSON.stringify(answer.plan)).catch(() => {});
    const foreground = AppState.currentState === 'active';
    covered = await syncIosActivities(answer.plan, answer.reminders, {
      foreground,
      mayPlan: true,
      origin: foreground ? 'foreground' : 'background',
    });
  } catch (err) {
    void logLine(
      'warn',
      `live activities not kept in step: ${err instanceof Error ? err.message : String(err)}`,
    );
  }
  return { items: answer.reminders, covered };
}

/**
 * The quick start on a return to the app: the full pass reads every account
 * live and can take seconds, and someone who tapped the reminder, glanced and
 * locked again would be gone before it started anything. So the plan kept from
 * the last pass starts a running chain at once; the full pass reconciles after.
 * It plans nothing ahead — that waits for fresh data.
 */
export function startFromLastPlan(): void {
  if (Platform.OS !== 'ios') return;
  // Also called on a launch straight into the background, where iOS refuses
  // every start: only a launch that is not in the background may try. An
  // early `inactive` still counts as in front.
  void passFromLastPlan('quick start', AppState.currentState !== 'background');
}

/**
 * The first step of a background round: the same pass from the last plan,
 * without the network, before the round reads every account. It ends what
 * is over and changes what runs; it starts and plans nothing.
 *
 * A round gets about thirty seconds from iOS, often less, and its full pass
 * comes last, behind the providers. On Toni's iPhone (build 71, 2026-11-17)
 * iOS woke Aperio ten minutes after an appointment ended, the round died on
 * hung requests before its pass, and the activity said "over" for two more
 * hours. Never throws.
 */
export function endFromLastPlan(): Promise<void> {
  if (Platform.OS !== 'ios') return Promise.resolve();
  return passFromLastPlan('round start', false);
}

async function passFromLastPlan(origin: ActivityPassOrigin, foreground: boolean): Promise<void> {
  try {
    const raw = await AsyncStorage.getItem(PLAN_KEY).catch(() => null);
    if (raw == null) return;
    const plan = JSON.parse(raw) as LiveActivityPlan;
    await syncIosActivities(plan, [], { foreground, mayPlan: false, origin });
  } catch (err) {
    void logLine(
      'warn',
      `live activity ${origin} failed: ${err instanceof Error ? err.message : String(err)}`,
    );
  }
}
