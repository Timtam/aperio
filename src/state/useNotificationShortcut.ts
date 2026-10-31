import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  isCommandError,
  notificationShortcutCreate,
  notificationShortcutRemove,
  notificationShortcutStatus,
  type NotificationShortcutStatus,
} from '../api/client';

export type NotificationShortcutPhase =
  | { kind: 'loading' }
  | { kind: 'ready'; status: NotificationShortcutStatus }
  | { kind: 'probeFailed'; error: string };

type Action = 'setUp' | 'remove';

/** The status sentence for each state that is shown at all. */
export const NOTIFICATION_SHORTCUT_STATUS_KEY = {
  dev_build: 'dialogs.notificationShortcut.statusDevBuild',
  not_set_up: 'dialogs.notificationShortcut.statusNotSetUp',
  foreign: 'dialogs.notificationShortcut.statusForeign',
  set_up: 'dialogs.notificationShortcut.statusSetUp',
  points_elsewhere: 'dialogs.notificationShortcut.statusPointsElsewhere',
} as const satisfies Record<Exclude<NotificationShortcutStatus['kind'], 'unsupported'>, string>;

function messageOf(err: unknown): string {
  if (isCommandError(err)) return err.message;
  return err instanceof Error ? err.message : String(err);
}

/**
 * The Start menu shortcut Windows needs before it shows Aperio's
 * notifications: its status, read from disk on mount (the shortcut can be
 * deleted outside Aperio), and setting it up or removing it.
 *
 * One action at a time; a second press while one runs does nothing. A
 * success says so once (focus stays on the button, whose name NVDA would
 * not read again); a failure shows one alert line instead and reads the
 * status again, so the page shows what is really on disk. `onSetUp` runs
 * after a successful set-up (the hint closes itself).
 */
export function useNotificationShortcut(options: { onSetUp?: () => void } = {}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const [phase, setPhase] = useState<NotificationShortcutPhase>({ kind: 'loading' });
  const [busy, setBusy] = useState<Action | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busyRef = useRef(false);
  const mounted = useRef(true);
  const onSetUpRef = useRef(options.onSetUp);
  onSetUpRef.current = options.onSetUp;

  const probe = useCallback(async () => {
    try {
      const status = await notificationShortcutStatus();
      if (mounted.current) setPhase({ kind: 'ready', status });
    } catch (err) {
      if (mounted.current) setPhase({ kind: 'probeFailed', error: messageOf(err) });
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void probe();
    return () => {
      mounted.current = false;
    };
  }, [probe]);

  const run = useCallback(
    async (action: Action) => {
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(action);
      setError(null);
      try {
        const status =
          action === 'setUp'
            ? await notificationShortcutCreate()
            : await notificationShortcutRemove();
        if (!mounted.current) return;
        setPhase({ kind: 'ready', status });
        announce(
          t(
            action === 'setUp'
              ? 'dialogs.notificationShortcut.setUpDone'
              : 'dialogs.notificationShortcut.removeDone',
          ),
        );
        if (action === 'setUp') onSetUpRef.current?.();
      } catch (err) {
        if (!mounted.current) return;
        setError(t('dialogs.notificationShortcut.failed', { error: messageOf(err) }));
        void probe();
      } finally {
        busyRef.current = false;
        if (mounted.current) setBusy(null);
      }
    },
    [announce, probe, t],
  );

  return {
    phase,
    busy,
    error,
    setUp: useCallback(() => void run('setUp'), [run]),
    remove: useCallback(() => void run('remove'), [run]),
  };
}
