import * as Notifications from 'expo-notifications';

import { CHANNEL_ID, ensureAndroidChannel, ensurePermission } from '../reminders/scheduler';

/**
 * Fire a best-effort, IMMEDIATE OS-level notification — the mobile twin of the
 * desktop `state/notify.ts`.
 *
 * Routes through the SAME permission + Android-channel path the reminder
 * scheduler already uses (`ensurePermission` / `ensureAndroidChannel`), so the
 * user is asked for notification permission at most once across both delivery
 * paths and the banner lands on the existing "Reminders" channel. A denied
 * permission (or any failure in the expo-notifications bridge) is swallowed:
 * notifications are a secondary channel, and every caller still surfaces the
 * same information through an in-app `AccessibilityInfo.announceForAccessibility`
 * live announcement, so a suppressed OS notification never hides anything.
 *
 * Fired with a channel-aware immediate trigger (`{ channelId }`): like
 * `trigger: null` it delivers right away, and it routes to the existing
 * "Reminders" channel on Android (the channel is ignored on iOS, which has
 * none). The scheduler's `setNotificationHandler` shows the banner even when
 * the app is foregrounded. `context` only tags the warning logged on failure
 * so a call site is identifiable; it never reaches the OS.
 */
export async function notify(
  title: string,
  body: string,
  context = 'notification',
): Promise<void> {
  try {
    if (!(await ensurePermission())) return;
    await ensureAndroidChannel();
    await Notifications.scheduleNotificationAsync({
      content: { title, body, sound: 'default' },
      // Channel-aware immediate trigger — fires now, on the default channel.
      trigger: { channelId: CHANNEL_ID },
    });
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn(`${context} failed`, err);
  }
}
