import { allDayReminderDays } from '@aperio/shared';
import * as Notifications from 'expo-notifications';
import { useEffect } from 'react';
import { AppState, Platform } from 'react-native';

import CalFfi from '../../modules/cal-ffi';
import i18n from '../../i18n';
import { UpcomingReminder, upcomingReminders } from '../api/reminders';
import { customSoundPath } from '../api/sounds';
import { getHiddenCalendars } from '../state/calendarVisibility';
import { setRemindersRefreshHook } from '../state/cacheObserver';
import { whenStartupSettled } from '../state/startupGate';
import { upcomingDayStartNotifications } from './dayStartSchedule';

const DAY_MS = 86_400_000;

/** The notification body for a trigger. An all-day event reads as "Ganztägig"
 *  (single day) or "Ganztägig · 24. Juni bis 26. Juni" (multi-day) — spelled-out,
 *  localized — instead of the bare "00:00" a midnight start would format to.
 *  Timed events and tasks keep the Rust-built body (the time). */
function notificationBody(r: UpcomingReminder): string {
  if (r.item_kind !== 'event' || !r.all_day) return r.body;
  const days = allDayReminderDays(r.start, r.end);
  if (days <= 1) return i18n.t('dialogs.reminders.allDay');
  // `end` is exclusive (the next midnight), so step back one day for the
  // inclusive last day the user sees.
  const lastDay = new Date(new Date(r.end).getTime() - DAY_MS);
  const fmt = new Intl.DateTimeFormat(i18n.language, {
    day: 'numeric',
    month: 'long',
  });
  return i18n.t('dialogs.reminders.allDayRange', {
    from: fmt.format(new Date(r.start)),
    to: fmt.format(lastDay),
  });
}

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
export const CHANNEL_ID = 'reminders';
/** Silent variant — a "Silent" reminder (§14.4) lands here: visible in the
 *  shade, no sound. Android couples sound to the channel (immutable per-channel),
 *  so a no-sound reminder needs its own LOW-importance channel rather than a
 *  per-notification flag. */
const SILENT_CHANNEL_ID = 'reminders-silent';
/** Per-custom-sound channel id prefix (`reminders-custom-<sha256>`). Android
 *  binds a channel's sound at creation (immutable), so each custom sound gets
 *  its own create-once channel pointing at the imported file. */
const CUSTOM_CHANNEL_PREFIX = 'reminders-custom-';
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
/** Create the Android notification channels (create-once; no-op on iOS or after
 *  the first call). Exported so the day-start notifier delivers on the same
 *  default channel without duplicating the dance. */
export async function ensureAndroidChannel(): Promise<void> {
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

/** Resolve a reminder's OS delivery — which Android channel + the per-notification
 *  sound (iOS). "Silent" → the LOW silent channel / no sound. A CUSTOM sound on
 *  Android → a per-sound channel whose sound is the imported file (native
 *  ensureCustomSoundChannel over a FileProvider URI); if the file isn't on this
 *  device, the channel can't be made, or we're on iOS (which can't use a runtime
 *  file as a notification sound), it falls back to the default sound. Everything
 *  else → the default channel + default sound. Never throws. */
async function resolveDelivery(
  r: UpcomingReminder,
): Promise<{ channelId: string; sound: 'default' | false }> {
  const src = r.sound?.source;
  if (src?.type === 'silent') return { channelId: SILENT_CHANNEL_ID, sound: false };
  if (src?.type === 'custom' && Platform.OS === 'android') {
    try {
      const path = await customSoundPath(src.sha256);
      if (path != null) {
        const channelId = `${CUSTOM_CHANNEL_PREFIX}${src.sha256}`;
        // Distinguishing name (the short hash) so the per-sound channels don't
        // all read identically as "Reminders" in Android notification settings.
        const name = `${i18n.t('reminders.label')} (${src.sha256.slice(0, 8)})`;
        await CalFfi.ensureCustomSoundChannel(channelId, path, name);
        return { channelId, sound: 'default' };
      }
    } catch {
      // Fall through to the default channel/sound — a missing file or a failed
      // native channel must never lose the reminder.
    }
  }
  return { channelId: CHANNEL_ID, sound: 'default' };
}

/** Ensure OS notification permission, prompting once if we still can. Shared
 *  with the day-start reminder notifier (state/notify.ts) so both delivery
 *  paths go through one permission flow — the user is asked at most once. */
export async function ensurePermission(): Promise<boolean> {
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
    const hidden = await getHiddenCalendars();
    // Day-start "today's tasks" notifications, pre-computed for the same
    // horizon — the in-app day-start check only runs while the app is open,
    // so without these the 9:00 nudge arrived only when the user next opened
    // the app. Best-effort: a failure must never cost the explicit reminders.
    const dayStarts = await upcomingDayStartNotifications(
      Math.floor(HORIZON_MINUTES / (24 * 60)),
    ).catch(() => []);
    // Cancel-then-reschedule: the cheapest way to stay consistent with the
    // current data (a deleted/rescheduled item simply isn't in the new list).
    await Notifications.cancelAllScheduledNotificationsAsync();
    const now = Date.now();
    // The day-start nudges are the headline reminders — schedule them first
    // and cap the explicit ones so the total stays under the OS pending limit.
    for (const ds of dayStarts) {
      await Notifications.scheduleNotificationAsync({
        content: {
          title: i18n.t('dialogs.dayStartReview.reminders.notificationTitle'),
          body: i18n.t('dialogs.dayStartReview.reminders.notificationBody', {
            count: ds.count,
          }),
          sound: 'default',
          // No itemId: a tap just opens the app, where the live in-app
          // day-start review (announce + dialog) takes over with fresh data.
          data: { dayStart: true },
        },
        trigger: {
          type: Notifications.SchedulableTriggerInputTypes.DATE,
          date: ds.triggerAt,
          channelId: CHANNEL_ID,
        },
      });
    }
    const due = items
      .filter((r) => new Date(r.trigger_at).getTime() > now)
      // Hidden calendars (the Calendars-screen toggles) silence their event
      // reminders; tasks are unaffected (visibility is calendar-only).
      .filter((r) => !(r.item_kind === 'event' && hidden.has(r.container_id)))
      .slice(0, Math.max(0, MAX_SCHEDULED - dayStarts.length));
    for (const r of due) {
      // §14.4: the channel carries the sound on Android (silent / custom / default);
      // iOS has no channels, so the per-notification `sound` is what matters there
      // (Silent → false, else the default — iOS can't play a runtime custom file).
      // Per-notification VOLUME isn't an OS concept (notifications use the system
      // volume), so SoundConfig.volume is N/A.
      const { channelId, sound } = await resolveDelivery(r);
      await Notifications.scheduleNotificationAsync({
        content: {
          title: r.title,
          body: notificationBody(r),
          sound,
          // Carried so a tap can route to the item (wired in App).
          data: { itemId: r.item_id, itemKind: r.item_kind },
        },
        trigger: {
          type: Notifications.SchedulableTriggerInputTypes.DATE,
          date: new Date(r.trigger_at),
          channelId,
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

// Re-plan whenever fresh data lands outside a local mutation: a peer sync
// round applied changes (a desktop-created task can add/remove reminders or
// change the day-start counts) or an external-cache refresh finished.
// Registered as a callback because cacheObserver importing this module
// directly would close a module cycle through the api layer.
setRemindersRefreshHook(refreshRemindersSoon);

/** Mount once near the app root: reschedule on launch + every foreground-resume
 *  (the latter catches reminders synced in from a peer while we were away).
 *  The LAUNCH run is startup-gated: `upcomingRemindersJson` is a heavy
 *  trigger-enumeration pass on the serial native queue, and the OS
 *  notifications it plans sit days ahead — running it ~1.5s after first
 *  paint changes nothing for the user but keeps the queue clear for the
 *  visible screen's first read. Foreground resumes pass straight through
 *  (the gate is open by then). */
export function useReminderTriggers(): void {
  useEffect(() => {
    whenStartupSettled('reminders', () => void rescheduleReminders());
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') void rescheduleReminders();
    });
    return () => sub.remove();
  }, []);
}
