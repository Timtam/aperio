import {
  useCallback,
  useEffect,
  useId,
  useRef,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
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
 * ancestor; the portal target is `document.body`. Because `App.tsx`
 * keeps `role="application"` at the very top, the portaled dialog
 * still lives *inside* that application boundary.
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
}

export function Modal({
  isOpen,
  onClose,
  title,
  children,
  className,
  dismissOnBackdrop = true,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  const titleId = useId();

  // Open: snapshot the currently focused element, focus the first
  // focusable inside the dialog body. The header's close button is
  // intentionally skipped — landing on it would be jarring; users
  // expect to start typing immediately. Escape still closes from any
  // focus point. Close: restore the previous focus.
  useEffect(() => {
    if (!isOpen) return;
    previouslyFocused.current = document.activeElement as HTMLElement | null;

    const dialog = dialogRef.current;
    if (dialog) {
      const body = dialog.querySelector<HTMLElement>('.modal__body') ?? dialog;
      const focusables = getFocusables(body);
      const first = focusables[0] ?? dialog;
      first.focus({ preventScroll: true });
    }

    return () => {
      previouslyFocused.current?.focus({ preventScroll: true });
    };
  }, [isOpen]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key !== 'Tab') return;

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
        <div className="modal__body">{children}</div>
      </div>
    </div>,
    document.body,
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
