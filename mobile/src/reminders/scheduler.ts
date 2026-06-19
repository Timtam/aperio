import * as Notifications from 'expo-notifications';
import { useEffect } from 'react';
import { AppState, Platform } from 'react-native';

import i18n from '../../i18n';
import { upcomingReminders } from '../api/reminders';

// Mobile reminder delivery. The desktop fires reminders from a live tokio
// worker that sleeps until the next trigger; iOS suspends background JS, so
// that model can't run on-device. Instead we SCHEDULE the upcoming triggers
// ahead-of-time with the OS (expo-notifications), which delivers them even when
// the app is killed, and reschedule on launch + foreground-resume + after a
// mutation. WHAT fires WHEN comes from the shared Rust core via cal-ffi
// (`upcomingReminders`) — one source of truth with the desktop scheduler; this
// layer only owns OS delivery. All operations are best-effort + silent.

/** How far ahead we schedule. 7 days bounds the pending set while comfortably
 *  covering the cadence a personal task/calendar app needs. */
const HORIZON_MINUTES = 7 * 24 * 60;
/** Cap pending notifications well under iOS's ~64-per-app limit (older ones are
 *  dropped silently past it). The soonest N are kept; a later reschedule (next
 *  foreground/mutation) rolls the window forward. */
const MAX_SCHEDULED = 60;
const CHANNEL_ID = 'reminders';
/** Silent variant — a "Silent" reminder (§14.4) lands here: visible in the
 *  shade, no sound. Android couples sound to the channel (immutable per-channel),
 *  so a no-sound reminder needs its own LOW-importance channel rather than a
 *  per-notification flag. */
const SILENT_CHANNEL_ID = 'reminders-silent';
/** Debounce for the post-mutation reschedule — a burst of edits coalesces. */
const DEBOUNCE_MS = 2500;

// When a reminder arrives while the app is foregrounded, still surface it —
// but honour a "Silent" reminder by not playing its sound (the scheduled
// notification set content.sound=false; mirror that in the foreground handler).
Notifications.setNotificationHandler({
  handleNotification: async (notification) => ({
    shouldShowBanner: true,
    shouldShowList: true,
    shouldPlaySound: notification.request.content.sound != null,
    shouldSetBadge: false,
  }),
});

let channelEnsured = false;
async function ensureAndroidChannel(): Promise<void> {
  if (Platform.OS !== 'android' || channelEnsured) return;
  // Android 8+ requires a channel; its name is what the user sees in system
  // settings to customise sound/importance per-app.
  await Notifications.setNotificationChannelAsync(CHANNEL_ID, {
    name: i18n.t('reminders.label'),
    importance: Notifications.AndroidImportance.HIGH,
  });
  // A second channel for "Silent" reminders — LOW importance plays no sound
  // (still shown in the shade). The user can fine-tune both in system settings.
  await Notifications.setNotificationChannelAsync(SILENT_CHANNEL_ID, {
    name: i18n.t('reminders.silentLabel'),
    importance: Notifications.AndroidImportance.LOW,
  });
  channelEnsured = true;
}

/** Whether this reminder's effective sound is "silent" (visual only). Anything
 *  else (system, or a custom sound we can't bundle yet) plays the default. */
function isSilent(r: { sound?: { source?: { type?: string } } }): boolean {
  return r.sound?.source?.type === 'silent';
}

async function ensurePermission(): Promise<boolean> {
  const current = await Notifications.getPermissionsAsync();
  if (current.granted) return true;
  if (!current.canAskAgain) return false;
  const next = await Notifications.requestPermissionsAsync();
  return next.granted;
}

let inFlight = false;

/** Re-read the upcoming reminders from the core and replace the scheduled OS
 *  notifications with the soonest `MAX_SCHEDULED` future ones. Idempotent +
 *  guarded against overlap; never throws. */
export async function rescheduleReminders(): Promise<void> {
  if (inFlight) return;
  inFlight = true;
  try {
    if (!(await ensurePermission())) return;
    await ensureAndroidChannel();
    const items = await upcomingReminders(HORIZON_MINUTES);
    // Cancel-then-reschedule: the cheapest way to stay consistent with the
    // current data (a deleted/rescheduled item simply isn't in the new list).
    await Notifications.cancelAllScheduledNotificationsAsync();
    const now = Date.now();
    const due = items
      .filter((r) => new Date(r.trigger_at).getTime() > now)
      .slice(0, MAX_SCHEDULED);
    for (const r of due) {
      const silent = isSilent(r);
      await Notifications.scheduleNotificationAsync({
        content: {
          title: r.title,
          body: r.body,
          // §14.4: "Silent" plays no sound (visual only); anything else uses the
          // OS default. iOS honours this per-notification field; Android takes it
          // from the channel (below). Per-notification VOLUME isn't an OS concept
          // — notifications use the system volume — so SoundConfig.volume is N/A.
          sound: silent ? false : 'default',
          // Carried so a tap can route to the item (wired in App).
          data: { itemId: r.item_id, itemKind: r.item_kind },
        },
        trigger: {
          type: Notifications.SchedulableTriggerInputTypes.DATE,
          date: new Date(r.trigger_at),
          channelId: silent ? SILENT_CHANNEL_ID : CHANNEL_ID,
        },
      });
    }
  } catch {
    // Best-effort: a permission denial / bridge hiccup must never crash a
    // background reschedule. The next trigger (launch/foreground) retries.
  } finally {
    inFlight = false;
  }
}

let timer: ReturnType<typeof setTimeout> | null = null;

/** Debounced reschedule — call after a local mutation (a burst of edits
 *  coalesces into one reschedule `DEBOUNCE_MS` after the last). */
export function refreshRemindersSoon(): void {
  if (timer != null) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void rescheduleReminders();
  }, DEBOUNCE_MS);
}

/** Mount once near the app root: reschedule on launch + every foreground-resume
 *  (the latter catches reminders synced in from a peer while we were away). */
export function useReminderTriggers(): void {
  useEffect(() => {
    void rescheduleReminders();
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') void rescheduleReminders();
    });
    return () => sub.remove();
  }, []);
}
