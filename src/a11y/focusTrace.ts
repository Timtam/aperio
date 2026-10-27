/**
 * A temporary trace of where focus goes, into the log (`console.info` reaches
 * the log file as `aperio::webview` in every build). Toni, 2026-10-26: after a
 * file import opened with Ctrl+O, NVDA hung between focus and browse mode, and
 * the code shows no cause. For `ms` after it starts, every focus change and the
 * window gaining or losing the keyboard is written with where the element is
 * — no text of it, so no appointment's title reaches the log. Remove once the
 * cause is known (TODO).
 */
export function traceFocus(label: string, ms: number): void {
  const started = performance.now();
  const say = (what: string, target: EventTarget | null) => {
    const at = Math.round(performance.now() - started);
    // eslint-disable-next-line no-console
    console.info(
      `[focus ${label} +${at}ms] ${what}: ${describe(target)} hasFocus=${document.hasFocus()}`,
    );
  };
  // A focused element that is removed fires no focusout: focus drops to the
  // body silently. A look every half second catches that.
  let seen: Element | null = document.activeElement;
  const onFocusIn = (event: FocusEvent) => {
    seen = document.activeElement;
    say('focusin', event.target);
  };
  const onFocusOut = (event: FocusEvent) =>
    say(`focusout to ${describe(event.relatedTarget)}`, event.target);
  const onWindowFocus = () => say('window focus', document.activeElement);
  const onWindowBlur = () => say('window blur', document.activeElement);
  document.addEventListener('focusin', onFocusIn, true);
  document.addEventListener('focusout', onFocusOut, true);
  window.addEventListener('focus', onWindowFocus);
  window.addEventListener('blur', onWindowBlur);
  const poll = setInterval(() => {
    if (document.activeElement === seen) return;
    seen = document.activeElement;
    say('now', seen);
  }, 500);
  say('start', document.activeElement);
  setTimeout(() => {
    clearInterval(poll);
    say('end', document.activeElement);
    document.removeEventListener('focusin', onFocusIn, true);
    document.removeEventListener('focusout', onFocusOut, true);
    window.removeEventListener('focus', onWindowFocus);
    window.removeEventListener('blur', onWindowBlur);
  }, ms);
}

/** Where an element is, by its tag, role, id and first class, and whether it
 *  sits in the application, a dialog, or an inert subtree. */
export function describe(target: EventTarget | null): string {
  if (!(target instanceof Element)) return target === null ? 'null' : 'not an element';
  if (target === document.body) return 'body';
  if (target === document.documentElement) return 'html';
  const role = target.getAttribute('role');
  const firstClass = target.classList[0];
  const where = [
    target.closest('#app-root') ? 'in app-root' : 'OUTSIDE app-root',
    target.closest('[role="dialog"]') ? 'in dialog' : null,
    target.closest('[inert]') ? 'INERT' : null,
    target.isConnected ? null : 'DETACHED',
  ].filter(Boolean);
  return (
    target.tagName.toLowerCase() +
    (role ? `[role=${role}]` : '') +
    (target.id ? `#${target.id}` : '') +
    (firstClass ? `.${firstClass}` : '') +
    ` (${where.join(', ')})`
  );
}
