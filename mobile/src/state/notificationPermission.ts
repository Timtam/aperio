import { useFocusEffect } from '@react-navigation/native';
import * as Notifications from 'expo-notifications';
import { useCallback, useEffect, useState } from 'react';
import { AppState } from 'react-native';

/**
 * Whether the phone lets Aperio show its reminders: allowed, not allowed
 * but still askable, or not allowed with only the system settings left to
 * allow it (the user declined, or iOS asked once already).
 *
 * The phone twin of the desktop's Windows-notifications section: without
 * the permission every reminder Aperio schedules is dropped by the OS, and
 * nothing in the app said so. The scheduler (`ensurePermission`) asks at
 * most once on its own; this is where the user sees the state.
 */
export type NotificationPermission = 'granted' | 'askable' | 'blocked';

export async function readNotificationPermission(): Promise<NotificationPermission> {
  const current = await Notifications.getPermissionsAsync();
  if (current.granted) return 'granted';
  return current.canAskAgain ? 'askable' : 'blocked';
}

/**
 * The permission, read again whenever the screen comes into view and when
 * the app returns to the foreground (the user may have changed it in the
 * system settings). `null` while unknown or when it could not be read.
 */
export function useNotificationPermission(): {
  permission: NotificationPermission | null;
  refresh: () => Promise<NotificationPermission | null>;
} {
  const [permission, setPermission] = useState<NotificationPermission | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await readNotificationPermission();
      setPermission(next);
      return next;
    } catch {
      setPermission(null);
      return null;
    }
  }, []);

  useFocusEffect(
    useCallback(() => {
      void refresh();
    }, [refresh]),
  );

  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') void refresh();
    });
    return () => sub.remove();
  }, [refresh]);

  return { permission, refresh };
}
