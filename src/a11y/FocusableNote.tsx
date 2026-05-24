import type { ReactNode } from 'react';

/**
 * A focusable paragraph that NVDA reads aloud when it receives
 * focus.
 *
 * Solves a specific Modal + screen-reader interaction we hit
 * across the Settings dialog:
 *
 * 1. `Modal` wraps its body in `role="application"` so NVDA stays
 *    in focus mode for the whole dialog — necessary so form
 *    fields behave like form fields (see Modal.tsx for the
 *    reasoning).
 *
 * 2. NVDA in focus mode only stops on focusable elements. Static
 *    `<p>` paragraphs are invisible to that traversal, so any
 *    explanatory prose in a Settings panel was unreadable to NVDA
 *    users unless it was tied via `aria-describedby` to some
 *    other focusable control downstream.
 *
 * 3. Naively adding `tabIndex={0}` to a paragraph made the
 *    element focusable but NVDA still announced "Anmerkung" /
 *    "Note" (when we tried `role="note"`) without speaking the
 *    text content — because `role="note"` is a structural role
 *    that doesn't compute its accessible name from children, and
 *    a bare focusable `<p>` doesn't either.
 *
 * The fix: drop the role and put the text in `aria-label` so
 * NVDA reads it as the focused element's accessible name. The
 * visible text and the aria-label carry the same string, which
 * means screen-reader users hear exactly what sighted users see.
 *
 * Use this anywhere a Settings (or other Modal-hosted) panel
 * has explanatory prose the user might want to read in
 * sequence. For prose that's already tied to a downstream
 * control via `aria-describedby`, this component is redundant
 * — the control's focus event already announces the text.
 *
 * Signature: `children` is `string` rather than `ReactNode`
 * because the whole point is that the text becomes the
 * accessible name; JSX children would defeat the i18n / TTS
 * round-trip we're trying to preserve.
 */
export interface FocusableNoteProps {
  /** The note text. Used both as the visible content AND as
   *  the focusable element's `aria-label`. */
  children: string;
  /** Optional class for layout (the parent decides whether this
   *  reads as a hint, a warning, …). */
  className?: string;
  /** Optional id so a downstream control can still use this
   *  paragraph via `aria-describedby` — the focusable +
   *  describedby paths aren't mutually exclusive. */
  id?: string;
}

export function FocusableNote({
  children,
  className,
  id,
}: FocusableNoteProps): ReactNode {
  return (
    <p
      id={id}
      tabIndex={0}
      // `aria-label` mirrors the visible text. Without it, NVDA
      // in focus mode either falls back to the role (silent for
      // bare `<p>`, or "Anmerkung" with `role="note"`) or
      // announces nothing. The duplication isn't ideal but it
      // gives screen-reader and sighted users the same content.
      aria-label={children}
      className={className}
    >
      {children}
    </p>
  );
}
