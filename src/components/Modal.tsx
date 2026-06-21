import {
  useCallback,
  useEffect,
  useId,
  useRef,
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
}

export function Modal({
  isOpen,
  onClose,
  title,
  children,
  className,
  dismissOnBackdrop = true,
  initialFocusRef,
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
  useEffect(() => {
    if (!isOpen) return;
    const candidate = document.activeElement;
    previouslyFocused.current =
      candidate instanceof HTMLElement && candidate !== document.body
        ? candidate
        : null;

    const dialog = dialogRef.current;
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
        className={'modal' + (className ? ' ' + className : '')}
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
