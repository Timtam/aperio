/**
 * Whether the user is in the middle of something the dialog stack does not
 * know about: a view's own modal, or a text field in use. What the backend
 * asks to open on its own (the notification hint, a clicked reminder) waits
 * for it.
 */
export function pageBusy(): boolean {
  return modalOpen() || fieldInUse();
}

/** A modal dialog is on the page: the stack's own, or a view's. */
export function modalOpen(): boolean {
  return document.querySelector('[role="dialog"][aria-modal="true"]') != null;
}

/** A text field, select or editable element has focus, such as the
 *  sidebar's rename, which saves on blur. */
export function fieldInUse(): boolean {
  const el = document.activeElement;
  return (
    el instanceof HTMLElement &&
    el.matches('input, textarea, select, [contenteditable="true"]')
  );
}
