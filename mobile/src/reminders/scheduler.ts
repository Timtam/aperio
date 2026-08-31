import { allDayReminderDays } from '@aperio/shared';
import * as Notifications from 'expo-notifications';
import { useEffect } from 'react';
import { AppState, Platform } from 'react-native';

import CalFfi from '../../modules/cal-ffi';
import i18n from '../../i18n';
import { logLine } from '../api/logs';
import { UpcomingReminder, upcomingReminders } from '../api/reminders';
import { customSoundPath } from '../api/sounds';
import { getHiddenCalendars } from '../state/calendarVisibility';
import { setRemindersRefreshHook } from '../state/cacheObserver';
import { settleExternalCaches } from '../state/cacheSettle';
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

/**
 * The once-per-session external-cache settle every reschedule waits behind.
 *
 * `rescheduleReminders` is cancel-and-rewrite: it drops EVERY pending OS
 * notification and re-plans from what the cache-only reads return right now.
 * At a cold start that read races the device-reminders bridge (it installs
 * right after the Host opens), so the first pass counted ZERO device
 * reminders into every pre-scheduled day-start notification — and when the
 * app was closed again before a post-warm replan ran, that undercount stood
 * for the whole 7-day horizon: "2 Aufgaben brauchen heute deine
 * Aufmerksamkeit" on a morning with three overdue device reminders. Waiting
 * for the same settle the in-app review uses keeps the PREVIOUS (good)
 * schedule in place until today's data is actually readable; a quick
 * open/close now changes nothing instead of wrecking the week.
 *
 * Anchored HERE — not on the launch trigger — because the startup gate
 * coalesces same-key callbacks: a cache update flushed while the gate is
 * still closed replaces the parked launch run with a plain replan, which
 * would slip past a trigger-side settle. Inside the reschedule itself no
 * path can bypass it. One settle per session: later calls await the
 * already-resolved promise (a no-op), so mutation/foreground replans stay
 * as immediate as before. Mutations arriving DURING the settle are safe
 * even though their own call bails on `inFlight` — the pending first pass
 * reads the caches only after the settle, so it sees their writes.
 */
let launchSettle: Promise<void> | null = null;
function ensureLaunchSettle(): Promise<void> {
  launchSettle ??= settleExternalCaches()
    .then((settle) => {
      void logLine('info', `reminders: first reschedule waits on cache settle (${settle})`);
    })
    // A rejected promise here would poison EVERY later reschedule of the
    // session (they all await this cell) — settle is best-effort, proceed.
    .catch(() => {});
  return launchSettle;
}

/**
 * Declare the caches settled without running the wait — for the OS background
 * round, which refreshes the providers and waits on them ITSELF with a
 * platform-sized budget before rescheduling. Its headless JS context is fresh
 * (so the settle cell is empty), and a second settle there would kick another
 * warm pass and poll for up to a minute against iOS's ~30-second task budget —
 * an expired background task counts as a failure and costs future scheduling.
 * The Host it opened registered the device-calendar bridge synchronously, so
 * its cache reads serve at worst last-known rows, never the cold-start empties
 * the settle exists to prevent.
 */
export function markExternalCachesSettled(): void {
  launchSettle ??= Promise.resolve();
}

/** Re-read the upcoming reminders from the core and replace the scheduled OS
 *  notifications with the soonest `MAX_SCHEDULED` future ones. Idempotent +
 *  guarded against overlap; never throws. The first run of a session waits
 *  for the external caches to settle first (see ensureLaunchSettle). */
export async function rescheduleReminders(): Promise<void> {
  if (inFlight) return;
  inFlight = true;
  try {
    if (!(await ensurePermission())) return;
    await ensureLaunchSettle();
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
// Through the startup gate: a launch warm pass can flush a cache update
// while the gate is still closed (it rides toward its 5s cap on a busy
// bridge) — the replan then parks and runs once at gate-open instead of
// competing with the first paint. Post-open: plain pass-through.
setRemindersRefreshHook(() => whenStartupSettled('reminders', refreshRemindersSoon));

/** Mount once near the app root: reschedule on launch + every foreground-resume
 *  (the latter catches reminders synced in from a peer while we were away).
 *  The LAUNCH run is startup-gated: `upcomingRemindersJson` is a heavy
 *  trigger-enumeration pass on the serial native queue, and the OS
 *  notifications it plans sit days ahead — running it ~1.5s after first
 *  paint changes nothing for the user but keeps the queue clear for the
 *  visible screen's first read. The session's FIRST reschedule additionally
 *  waits for the external caches to settle before its cancel-and-rewrite
 *  (see ensureLaunchSettle). Foreground resumes pass straight through (the
 *  gate is open by then). */
export function useReminderTriggers(): void {
  useEffect(() => {
    whenStartupSettled('reminders', () => void rescheduleReminders());
    const sub = AppState.addEventListener('change', (state) => {
      // Also through the gate: the listener registers before the gate is
      // even armed, so an inactive→active flip DURING the launch window
      // (notification shade, app-switcher peek) would otherwise run the
      // heavy enumeration un-gated. Post-open this is a pass-through.
      if (state === 'active') {
        whenStartupSettled('reminders', () => void rescheduleReminders());
      }
    });
    return () => sub.remove();
  }, []);
}
