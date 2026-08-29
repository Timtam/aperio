import {
  useCallback,
  useEffect,
  useId,
  useRef,
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type RefObject,
} from 'react';
import { createPortal } from 'react-dom';

/**
 * Accessible modal dialog.
 *
 * Implements every ARIA requirement DESIGN.md section 3.2 lists for
 * dialogs: `role="dialog"`, `aria-modal="true"`, `aria-labelledby`
 * pointing at the title, Escape closes, Tab and Shift+Tab cycle inside
 * the dialog (focus trap), initial focus lands inside the dialog on
 * open, and the previously focused element is restored on close.
 *
 * Rendered via a portal so the dialog escapes any `overflow: hidden`
 * ancestor. The portal target is `#app-root` (the element that carries
 * `role="application"`) — **not** `document.body` — so the dialog stays
 * inside the application boundary. Portaling to body would push the
 * dialog out of the application tree and NVDA would fall back to browse
 * mode whenever the focus moved into the dialog.
 *
 * Focus-mode trick (lifted from Synology DSM): the outer container
 * keeps `role="dialog" aria-modal="true"` so the screen reader still
 * announces "dialog: <title>" on open. The body, where the actual
 * form lives, gets its own `role="application"`. Without the inner
 * application role, NVDA's default for dialog content is browse mode
 * even with `aria-modal="true"` and `inert` on the rest of the page.
 * With it, the screen reader stays in focus mode end-to-end and form
 * fields behave like regular form fields.
 */
export interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  /** Optional class for the dialog panel — used to size the form. */
  className?: string;
  /**
   * If `false`, clicks on the backdrop are ignored. Forms with
   * unsaved changes will want this; routine dialogs leave the default
   * `true`.
   */
  dismissOnBackdrop?: boolean;
  /**
   * Explicit element to focus on open, overriding the default "first
   * focusable in the body". A content-first dialog whose first focusable sits
   * deep in a list — or whose list is still loading when the dialog opens —
   * should point this at a stable top element (e.g. its intro, `tabIndex={-1}`),
   * so focus reliably lands inside the `role="application"` body instead of
   * falling back to the non-focusable dialog container and stranding the screen
   * reader in an undetermined mode.
   */
  initialFocusRef?: RefObject<HTMLElement | null>;
  /**
   * Id of an element inside the dialog whose text DESCRIBES it (a confirm
   * message, a warning, an intro). Wired to the `role="dialog"` element's
   * `aria-describedby`, so NVDA reads it as part of the dialog's own
   * open announcement. The body is `role="application"`, where a static
   * `<p>` is otherwise invisible to focus-mode traversal — this is the
   * ARIA-correct way to make that prose reachable without turning it into
   * a Tab stop (cf. `FocusableNote` for text that must be a stop).
   */
  describedById?: string;
}

export function Modal({
  isOpen,
  onClose,
  title,
  children,
  className,
  dismissOnBackdrop = true,
  initialFocusRef,
  describedById,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  const titleId = useId();

  // Open: snapshot the currently focused element and move focus into
  // the dialog body. Close: restore the snapshot (best-effort).
  //
  // Note: by the time this effect runs the caller has typically already
  // toggled `inert` on the rest of the page, which would have blurred
  // the trigger. If `activeElement` looks like `<body>` we skip the
  // snapshot — restoring to body would be worse than doing nothing.
  // Code that needs a reliable restore (e.g. `DialogState`) captures
  // the trigger synchronously *before* opening the dialog.
  //
  // StrictMode-proofing (the dev build double-invokes mount effects —
  // run, cleanup, run again, with the DOM staying put): both halves
  // must not treat that synthetic cycle as a real open/close.
  //   - The SNAPSHOT must not overwrite the real trigger on the second
  //     run: by then focus is already INSIDE the dialog, and restoring
  //     to a dialog-internal node after the real close is a no-op on a
  //     disconnected element — focus would fall to <body> and NVDA out
  //     of application mode.
  //   - The RESTORE must not fire on the synthetic cleanup at all: the
  //     dialog is still there, and yanking focus back to the trigger
  //     silently un-announces a dialog that IS open. That's harmless
  //     under DialogHost (the shell is inert, the yank fails) but
  //     stranded every dialog NESTED inside another one — the Settings
  //     account-edit dialog opened with focus left on its trigger. The
  //     dialog node being gone is what tells a real close apart.
  useEffect(() => {
    if (!isOpen) return;
    const dialog = dialogRef.current;
    const candidate = document.activeElement;
    if (!(dialog && candidate instanceof HTMLElement && dialog.contains(candidate))) {
      previouslyFocused.current =
        candidate instanceof HTMLElement && candidate !== document.body
          ? candidate
          : null;
    }

    if (dialog) {
      const body = dialog.querySelector<HTMLElement>('.modal__body') ?? dialog;
      // An explicit initial-focus target wins: a content-first dialog whose
      // first focusable sits deep in a list (or whose list is still loading on
      // open) would otherwise leave focus on the non-focusable dialog div and
      // strand the screen reader in an undetermined mode. Else the first
      // focusable, else the dialog container.
      const focusables = getFocusables(body);
      const target = initialFocusRef?.current ?? focusables[0] ?? dialog;
      target.focus({ preventScroll: true });
    }

    return () => {
      // Still in the document ⇒ StrictMode's synthetic cleanup, not a
      // close — leave focus where the dialog put it. (`dialog` is the
      // node this run saw; a real close removed it from the DOM before
      // this cleanup runs, a synthetic one did not.)
      if (dialog?.isConnected) return;
      const target = previouslyFocused.current;
      if (!target) return;
      // Defer past the current React commit so the caller's `inert`
      // cleanup has time to flush.
      queueMicrotask(() => {
        target.focus({ preventScroll: true });
      });
    };
  }, [isOpen, initialFocusRef]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        // Don't let Escape bubble to an ancestor modal. React portals bubble
        // events through the component tree, so a nested dialog (e.g. the
        // custom-color picker opened from the event editor) would otherwise
        // also close its parent. Escape should close only this dialog.
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== 'Tab') return;
      // Same for the focus-trap key — keep Tab handling inside this dialog so
      // a parent modal's trap doesn't fight the nested one.
      e.stopPropagation();

      // Focus trap: keep Tab/Shift+Tab inside the dialog.
      const dialog = dialogRef.current;
      if (!dialog) return;
      const focusables = getFocusables(dialog);
      if (focusables.length === 0) {
        e.preventDefault();
        return;
      }
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;

      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    },
    [onClose],
  );

  // Focus-recovery safety net (best-effort backstop). A dialog control the user
  // is on can be DISABLED (`disabled={busy}` during an async action) — the
  // browser blurs it to `<body>`, outside `#app-root`'s `role="application"`,
  // so NVDA silently leaves application mode and the dialog's own Escape/Tab
  // handlers (React listeners on this subtree) stop receiving keys. `onBlur`
  // (a root-delegated focusout listener) reliably catches that still-connected
  // case and we pull focus back to a stable in-dialog stop. It does NOT
  // reliably fire when a focused element UNMOUNTS (a step advance, a resolved
  // row, a wiped form): the removed node is already disconnected, so its
  // focusout has no path to React's root listener. Those transitions are
  // therefore handled explicitly at the call sites (a step-heading
  // useLayoutEffect, a reparkFocus, a pristine guard that prevents the unmount
  // altogether); this net is the backstop for anything they miss. A dialog that
  // moves focus itself lands on an in-dialog element, so `relatedTarget` is
  // inside the dialog and this never fires — explicit moves always win.
  const handleFocusOut = useCallback((e: FocusEvent<HTMLDivElement>) => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const next: EventTarget | null = e.relatedTarget;
    // Focus went somewhere real INSIDE the dialog → nothing to do.
    if (next instanceof HTMLElement && dialog.contains(next)) return;
    // Focus moved to another real element outside the dialog (shouldn't
    // happen while open — Tab is trapped — but never fight a deliberate
    // move to a concrete element).
    if (next instanceof HTMLElement && next !== document.body) return;
    // Otherwise focus was LOST (to <body> or nothing). Recover — but only if
    // the dialog is still mounted and open (a CLOSING dialog must let focus go
    // so `close()` can restore the trigger). Defer on requestAnimationFrame,
    // not a microtask: a dialog that reparks focus deliberately (a step-heading
    // useLayoutEffect, a reparkFocus rAF scheduled synchronously in its click
    // handler) runs its move BEFORE this frame callback, so by the time this
    // fires focus is already back inside and the guard below bails. That makes
    // this a true last resort for the cases nobody handled, never a competitor
    // that yanks focus to the × button for a frame first.
    requestAnimationFrame(() => {
      const d = dialogRef.current;
      if (!d || !d.isConnected) return;
      const active = document.activeElement;
      if (active instanceof HTMLElement && d.contains(active)) return; // already recovered
      const focusables = getFocusables(d);
      // The header close button is always present, so there is always a stop.
      (focusables[0] ?? d).focus({ preventScroll: true });
    });
  }, []);

  const handleBackdrop = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      if (!dismissOnBackdrop) return;
      if (e.target === e.currentTarget) {
        onClose();
      }
    },
    [dismissOnBackdrop, onClose],
  );

  if (!isOpen) return null;

  const portalTarget =
    (typeof document !== 'undefined' &&
      document.getElementById('app-root')) ||
    (typeof document !== 'undefined' ? document.body : null);
  if (!portalTarget) return null;

  return createPortal(
    <div
      className="modal-backdrop"
      onClick={handleBackdrop}
      onKeyDown={handleKeyDown}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={describedById}
        className={'modal' + (className ? ' ' + className : '')}
        onBlur={handleFocusOut}
      >
        <header className="modal__header">
          <h2 id={titleId} className="modal__title">
            {title}
          </h2>
          <button
            type="button"
            className="modal__close"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </header>
        <div role="application" className="modal__body">
          {children}
        </div>
      </div>
    </div>,
    portalTarget,
  );
}

/**
 * Gather focusable descendants in document order.
 *
 * Matches the standard list of natively focusable elements plus anything
 * carrying a non-negative `tabindex`. Excludes disabled controls and
 * aria-hidden trees. Visibility isn't checked here — modals don't tend
 * to render `display: none` controls inline, and jsdom has no layout
 * engine to consult anyway.
 */
function getFocusables(root: HTMLElement): HTMLElement[] {
  const selector = [
    'a[href]',
    'button',
    'input',
    'select',
    'textarea',
    '[tabindex]:not([tabindex="-1"])',
  ].join(',');
  const nodes = root.querySelectorAll<HTMLElement>(selector);
  const out: HTMLElement[] = [];
  nodes.forEach((el) => {
    if (el.hasAttribute('disabled')) return;
    if (el.getAttribute('aria-hidden') === 'true') return;
    out.push(el);
  });
  return out;
}
