import { useEffect } from 'react';

import { useDialogState } from '../state/DialogState';

/**
 * Wire the global dialog-open shortcuts.
 *
 *  - `N` → quick-add event
 *  - `Ctrl/Cmd+N` → full new-event dialog
 *  - `Ctrl/Cmd+Shift+N` → new-task dialog
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
    openSearch,
    openReminders,
    mode,
  } = useDialogState();

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
      if (cmd && e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        openTaskDialog(null);
        return;
      }
      if (cmd && !e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        openEventDialog(null);
        return;
      }
      if (!cmd && !e.shiftKey && !e.altKey && e.key === 'n') {
        e.preventDefault();
        openQuickAdd();
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
    openSearch,
    openReminders,
  ]);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return true;
  if (target.isContentEditable) return true;
  return false;
}
