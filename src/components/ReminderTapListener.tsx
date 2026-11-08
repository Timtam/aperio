import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { reminderTapOf, type ReminderTap } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  getEventById,
  getEvents,
  getTaskById,
  getTasks,
  REMINDER_TAP_EVENT,
  takeReminderTap,
} from '../api/client';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import { fieldInUse, modalOpen } from '../state/pageBusy';
import { resolveReminderTap } from '../state/reminderTapTarget';

/**
 * A click on a reminder's notification opens what the reminder was for: that
 * occurrence, that task, or the day-start review (TODO A5, DESIGN §14.3). The
 * backend shows the window and says a tap waits; this takes it and opens it.
 * Mount once.
 *
 * It opens when all of this holds, and waits otherwise:
 *  - Aperio's window has focus: NVDA reads a dialog only in the focused
 *    window. The click asks Windows for it; if Windows only flashes the
 *    taskbar, the tap opens once the user switches over.
 *  - The calendars are listed: whether a meeting is someone else's depends on
 *    its calendar, and the listing routes an external event's read.
 *  - Nothing holds the user. Only the top dialog is shown, so one pushed onto
 *    an editor would unmount it and lose its input: the tap waits, says so
 *    once, and opens when the dialog closes. A text field in use (the
 *    sidebar's rename saves on blur) holds it the same way, and the
 *    announcement names the field, not a dialog that is not there. Two
 *    dialogs only show what is there, and the tap opens on top of them: the
 *    reminders overview and the day-start review. Closing the item comes
 *    back to them; the review is never replaced, since it may have opened
 *    in the hidden window, or while the tap waited, unheard.
 *
 * A tap is taken once, so a remount or a reload does not open it again. The
 * event may have gone out before this listened (a click while the page
 * loaded), so after subscribing it asks whether one waits.
 */
export function ReminderTapListener() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { mode, openTappedEvent, openTaskDialog, openDayStartReview } = useDialogState();
  const { calendars, loading } = useCalendarStore();
  const [pending, setPending] = useState<ReminderTap | null>(null);
  const [focused, setFocused] = useState(false);
  // Bumped on every focus move, so a tap held back by `pageBusy` is tried
  // again once the user is done (a rename ends, a view's dialog closes).
  const [focusTick, setFocusTick] = useState(0);
  // The pending tap the wait was announced for, and why it waited: once each.
  const waitAnnounced = useRef<{ tap: ReminderTap; why: string } | null>(null);

  useEffect(() => {
    const bump = () => setFocusTick((n) => n + 1);
    document.addEventListener('focusin', bump);
    return () => document.removeEventListener('focusin', bump);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const take = async () => {
      try {
        const tap = reminderTapOf(await takeReminderTap());
        if (tap != null && !disposed) setPending(tap);
      } catch {
        // Nothing waits, or the backend is gone with the window.
      }
    };
    void listen(REMINDER_TAP_EVENT, () => {
      if (!disposed) void take();
    }).then(
      (stop) => {
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        void take();
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
    if (pending == null || !focused || loading) return;
    const overReview = mode.kind === 'dayStartReview';
    const overList = mode.kind === 'reminders';
    // The review a day-start click asks for is already in front.
    if (pending.kind === 'dayStart' && overReview) {
      setPending(null);
      return;
    }
    const dialogOpen = mode.kind !== 'none' || modalOpen();
    if (!overReview && !overList && (dialogOpen || fieldInUse())) {
      const why = dialogOpen ? 'dialogs.reminders.tapWaits' : 'dialogs.reminders.tapWaitsField';
      const said = waitAnnounced.current;
      if (said?.tap !== pending || said.why !== why) {
        waitAnnounced.current = { tap: pending, why };
        announce(t(why), 'polite');
      }
      return;
    }
    let cancelled = false;
    const tap = pending;
    void resolveReminderTap(tap, {
      calendars,
      getEventById,
      getEvents,
      getTaskById,
      getTasks,
    }).then((target) => {
      // A dialog opened, or another tap came, while the item was read: this
      // runs again for what is current.
      if (cancelled) return;
      setPending(null);
      switch (target.kind) {
        case 'event':
          openTappedEvent(target.event, {
            initialScope: target.initialScope,
            shownOccurrence: target.shownOccurrence,
          });
          return;
        case 'task':
          openTaskDialog(target.task);
          return;
        case 'dayStart':
          openDayStartReview();
          return;
        case 'notFound':
          announce(t('dialogs.reminders.tapNotFound'), 'assertive');
          return;
      }
    });
    return () => {
      cancelled = true;
    };
  }, [
    pending,
    focused,
    loading,
    mode.kind,
    focusTick,
    calendars,
    announce,
    t,
    openTappedEvent,
    openTaskDialog,
    openDayStartReview,
  ]);

  return null;
}
