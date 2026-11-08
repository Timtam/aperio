/**
 * Whether the user is in the middle of something the dialog stack does not
 * know about: a view's own modal, or a text field in use. What the backend
 * asks to open on its own (the notification hint, a clicked reminder) waits
 * for it.
 */
export function pageBusy(): boolean {
  if (document.querySelector('[role="dialog"][aria-modal="true"]')) return true;
  const el = document.activeElement;
  return (
    el instanceof HTMLElement &&
    el.matches('input, textarea, select, [contenteditable="true"]')
  );
}
