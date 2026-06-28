import { useEffect, useRef } from 'react';

import { localDateKey } from '../intl/dateKey';
import { useDialogState } from '../state/dialogStateContext';
import { useViewState } from '../state/viewStateContext';

/**
 * Wire the global dialog-open shortcuts.
 *
 *  Create actions follow one rule — Ctrl/Cmd = event, Alt = task; Shift
 *  opens the full editor, no Shift opens quick-add:
 *  - `Ctrl/Cmd+N` → quick-add event
 *  - `Ctrl/Cmd+Shift+N` → full new-event dialog
 *  - `Alt+N` → quick-add task
 *  - `Alt+Shift+N` → full new-task dialog
 *  - `Ctrl/Cmd+,` → Settings dialog (matches the platform convention
 *    used by Visual Studio Code, macOS apps and most modern desktops)
 *
 * All ignore keystrokes inside form controls so a stray `N` while
 * typing in a description field doesn't pop another dialog. The
 * shortcut to *edit* the focused item lives in each view, because only
 * the view knows what's currently focused.
 */
export function useDialogShortcuts(): void {
  const {
    openEventDialog,
    openTaskDialog,
    openQuickAdd,
    openQuickAddTask,
    openSearch,
    openReminders,
    openSettings,
    mode,
  } = useDialogState();
  const { anchor } = useViewState();
  // Read the view's focused day at keypress time via a ref, so the
  // global listener doesn't re-subscribe on every day-navigation.
  const anchorRef = useRef(anchor);
  anchorRef.current = anchor;

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (isEditableTarget(e.target)) return;
      // If a dialog is already open, don't stack another on top.
      if (mode.kind !== 'none') return;

      const cmd = e.ctrlKey || e.metaKey;

      if (cmd && !e.shiftKey && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        openSearch();
        return;
      }
      if (cmd && e.shiftKey && e.key.toLowerCase() === 'r') {
        e.preventDefault();
        openReminders();
        return;
      }
      // Create actions: Ctrl/Cmd = event, Alt = task; Shift = full editor,
      // no Shift = quick-add. (macOS note: Option+N composes ñ — the app
      // targets Windows primarily; users can rebind.)
      if (cmd && !e.altKey && e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        openEventDialog(null, {
          defaultDate: localDateKey(anchorRef.current),
        });
        return;
      }
      if (cmd && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        // Anchor explicitly to the focused day, mirroring the Ctrl+Shift+N
        // full-event path + the toolbar button (the dialog would fall back to
        // the same view anchor anyway, but be explicit).
        openQuickAdd({ defaultDate: localDateKey(anchorRef.current) });
        return;
      }
      if (e.altKey && !cmd && e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        openTaskDialog(null);
        return;
      }
      if (e.altKey && !cmd && !e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        openQuickAddTask();
        return;
      }
      // `,` is a stable e.key across layouts; we don't compare e.code
      // because that varies (KeyComma vs Comma vs others) and not all
      // keyboards have a dedicated comma key on the same physical
      // position.
      if (cmd && !e.shiftKey && e.key === ',') {
        e.preventDefault();
        openSettings();
        return;
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [
    mode.kind,
    openEventDialog,
    openTaskDialog,
    openQuickAdd,
    openQuickAddTask,
    openSearch,
    openReminders,
    openSettings,
  ]);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return true;
  if (target.isContentEditable) return true;
  return false;
}
