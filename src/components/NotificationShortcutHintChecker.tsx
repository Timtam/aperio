import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useRef, useState } from 'react';

import {
  NOTIFICATION_SHORTCUT_HINT_EVENT,
  notificationShortcutHintPending,
  notificationShortcutHintShown,
  notificationShortcutStatus,
} from '../api/client';
import { useDialogState } from '../state/dialogStateContext';

/**
 * Opens the one-time hint (`NotificationShortcutHintDialog`) when the
 * backend says a reminder came due while Windows would hide it. Mount once.
 *
 * It opens only when all of this holds, and waits otherwise:
 *  - Aperio's window has focus. After an autostart Aperio sits hidden in the
 *    tray, a reminder app's usual state; a dialog in a window nobody looks
 *    at would be gone before anyone read it, and NVDA reads a dialog only
 *    in the focused window.
 *  - No other dialog is open: the stack shows only its top frame, and a
 *    dialog pushed onto an editor would unmount it and lose its edits.
 *  - Setting up is still needed, read from disk again just before.
 *
 * Once per app start: the backend lets it be raised once, and this keeps
 * it from opening twice. The event may have gone out before this listened
 * (a reminder at start), so after subscribing it asks whether one waits.
 */
export function NotificationShortcutHintChecker() {
  const { mode, openNotificationShortcutHint } = useDialogState();
  const [pending, setPending] = useState(false);
  const [focused, setFocused] = useState(false);
  const shown = useRef(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen(NOTIFICATION_SHORTCUT_HINT_EVENT, () => {
      if (!disposed) setPending(true);
    }).then(
      async (stop) => {
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        try {
          if ((await notificationShortcutHintPending()) && !disposed) setPending(true);
        } catch {
          // The next reminder raises it again.
        }
      },
      () => {},
    );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const win = getCurrentWindow();
    void win.isFocused().then(
      (now) => {
        if (!disposed && now) setFocused(true);
      },
      () => {},
    );
    void win
      .onFocusChanged(({ payload }) => {
        if (!disposed) setFocused(payload);
      })
      .then(
        (stop) => {
          if (disposed) stop();
          else unlisten = stop;
        },
        () => {},
      );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!pending || !focused || mode.kind !== 'none' || shown.current) return;
    let cancelled = false;
    void (async () => {
      let needed: boolean;
      try {
        needed = (await notificationShortcutStatus()).needs_set_up;
      } catch {
        return;
      }
      // A dialog opened, or focus left, while the status was read: this
      // effect runs again once that is over.
      if (cancelled || shown.current) return;
      if (!needed) {
        setPending(false);
        return;
      }
      shown.current = true;
      setPending(false);
      void notificationShortcutHintShown().catch(() => {});
      openNotificationShortcutHint();
    })();
    return () => {
      cancelled = true;
    };
  }, [pending, focused, mode.kind, openNotificationShortcutHint]);

  return null;
}
