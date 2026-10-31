import { isPermissionGranted, requestPermission } from '@tauri-apps/plugin-notification';

import { notifySystem } from '../api/client';

/**
 * Fire a best-effort OS-level notification.
 *
 * Requests notification permission lazily on the first call — the
 * user sees the OS prompt at most once per install; subsequent calls
 * short-circuit via the already-granted check, so callers can share
 * this helper without double-prompting. A denied permission (or any
 * failure in the plugin bridge) is swallowed: notifications are a
 * secondary channel, and every caller still surfaces the same
 * information through an in-app `aria-live` announcement, so a
 * suppressed OS notification never hides anything from the user.
 *
 * `context` is only used to tag the warning logged on failure so a
 * given call site is identifiable; it never reaches the OS.
 *
 * Sent through the backend (`notify_system`) rather than the plugin's own
 * JavaScript call, so it asks for the system sound by the same rule as a
 * reminder: on Windows a notification without a sound name is silent.
 */
export async function notify(
  title: string,
  body: string,
  context = 'notification',
): Promise<void> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      const result = await requestPermission();
      granted = result === 'granted';
    }
    if (!granted) return;
    await notifySystem(title, body);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn(`${context} failed`, err);
  }
}
