import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';

/**
 * Global toast queue.
 *
 * Aperio uses toasts for one specific job today: surfacing an Undo
 * button after a silent task batch (the day-start review's
 * auto-today / auto-backlog carry-over). The screen-reader-only
 * announcer is enough for "X tasks moved to today" — but the user
 * needs a visible, keyboard-reachable handle to reverse it, and an
 * aria-live region does not give them one.
 *
 * Why a stand-alone component and not the announcer:
 *
 *  - Announcer's two live regions are `class="sr-only"`. They have
 *    no visual presence and can't host buttons.
 *  - Toasts need keyboard focus (an Undo button) without stealing
 *    focus from whatever the user is doing. The pattern is "the
 *    button is there if you Tab to it; otherwise it dismisses on
 *    its own after a few seconds."
 *  - Mouse-dismiss and keyboard-dismiss (Esc on a focused toast)
 *    need targets the announcer doesn't have.
 *
 * Accessibility shape:
 *
 *  - The stack wrapper carries `role="region"` + `aria-label` so
 *    NVDA / JAWS users can navigate into the toast list via the
 *    landmarks list.
 *  - Each toast renders as `role="status"` with `aria-live="polite"`
 *    — same urgency as the announcer's polite region. We DON'T add
 *    the announcer call ourselves because callers already announce
 *    via the existing `useAnnouncer()` hook (we don't want to
 *    double-announce).
 *  - The Undo button is just a regular `<button>` reachable by Tab
 *    cycling. No focus-trap — toasts are non-modal.
 *  - Auto-dismiss timer pauses on hover and on focus inside the
 *    toast. This is a WAI-ARIA APG recommendation for toast/snackbar
 *    patterns: a user who's reaching for the Undo button must not
 *    have it disappear under their cursor.
 *
 * Stacking + lifecycle:
 *
 *  - Up to 3 toasts visible at a time. New toasts push older ones
 *    out (FIFO). Three feels comfortable; the day-start review
 *    fires at most two silent batches (today + backlog) so the
 *    limit only matters if some other future caller piles on.
 *  - Default duration is 10s. The Undo handler is responsible for
 *    its own success / failure feedback — we dismiss the toast as
 *    soon as the user clicks Undo so they don't see a stale "5
 *    tasks moved" while the reverse is in flight.
 *  - Dismissing a toast manually (Esc, click ×, or click Undo)
 *    clears its timer; nothing else cleans up.
 */

export type ToastId = string;

export interface ToastUndo {
  /** Button label shown next to the message. Defaults to the
   *  translated `toast.undoLabel` if absent. */
  label?: string;
  /** Called when the user clicks Undo. Awaited so we can show
   *  spinner / error UX in a future iteration; for now we just
   *  dismiss the toast as soon as the click registers. Failures
   *  bubble out — the caller decides whether to log them. */
  action: () => Promise<void> | void;
}

export interface ToastInput {
  /** Free-form text shown to the user. Should already be
   *  translated by the caller — the provider doesn't know the
   *  caller's i18n namespace. */
  message: string;
  /** Optional Undo button. When absent the toast is informational
   *  only (and auto-dismisses on the same timer). */
  undo?: ToastUndo;
  /** Milliseconds to live. Default 10_000. Pass `0` for sticky —
   *  the toast persists until manually dismissed. Useful for
   *  errors that need acknowledgement; not used by the carry-over
   *  flow. */
  durationMs?: number;
}

interface Toast extends ToastInput {
  id: ToastId;
  /** Internal flag — set true while the Undo handler is running so
   *  we can show "Undoing…" instead of "Undo" and disable the
   *  button. Failures revert it to false. */
  undoInFlight: boolean;
}

interface ToastContextValue {
  showToast: (input: ToastInput) => ToastId;
  dismissToast: (id: ToastId) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

const DEFAULT_DURATION_MS = 10_000;
const MAX_VISIBLE = 3;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  // Stable id generator — `useId()` collides on re-mount of the
  // same provider tree, so we maintain a monotonically increasing
  // counter inside a ref instead.
  const nextIdRef = useRef(0);

  const dismissToast = useCallback((id: ToastId) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const showToast = useCallback(
    (input: ToastInput): ToastId => {
      nextIdRef.current += 1;
      const id = `toast-${nextIdRef.current}`;
      setToasts((prev) => {
        const next: Toast = {
          id,
          message: input.message,
          undo: input.undo,
          durationMs: input.durationMs ?? DEFAULT_DURATION_MS,
          undoInFlight: false,
        };
        // FIFO cap: drop the oldest entries when we'd overflow.
        const trimmed =
          prev.length + 1 > MAX_VISIBLE
            ? prev.slice(prev.length + 1 - MAX_VISIBLE)
            : prev;
        return [...trimmed, next];
      });
      return id;
    },
    [],
  );

  // Per-toast "mark undo in flight" used by ToastRow. Kept out of
  // the public context so callers can't poke at toast internals.
  const setUndoInFlight = useCallback(
    (id: ToastId, value: boolean) => {
      setToasts((prev) =>
        prev.map((t) => (t.id === id ? { ...t, undoInFlight: value } : t)),
      );
    },
    [],
  );

  const value = useMemo<ToastContextValue>(
    () => ({ showToast, dismissToast }),
    [showToast, dismissToast],
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <ToastStack
        toasts={toasts}
        onDismiss={dismissToast}
        setUndoInFlight={setUndoInFlight}
      />
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) {
    throw new Error('useToast must be used inside <ToastProvider>');
  }
  return ctx;
}

// ─────────────────────────────────────────────────────────────────
// Rendering
// ─────────────────────────────────────────────────────────────────

function ToastStack({
  toasts,
  onDismiss,
  setUndoInFlight,
}: {
  toasts: Toast[];
  onDismiss: (id: ToastId) => void;
  setUndoInFlight: (id: ToastId, value: boolean) => void;
}) {
  const { t } = useTranslation();
  if (toasts.length === 0) return null;

  return (
    <div
      role="region"
      aria-label={t('toast.regionLabel')}
      className="toast-stack"
    >
      {toasts.map((toast) => (
        <ToastRow
          key={toast.id}
          toast={toast}
          onDismiss={onDismiss}
          setUndoInFlight={setUndoInFlight}
        />
      ))}
    </div>
  );
}

function ToastRow({
  toast,
  onDismiss,
  setUndoInFlight,
}: {
  toast: Toast;
  onDismiss: (id: ToastId) => void;
  setUndoInFlight: (id: ToastId, value: boolean) => void;
}) {
  const { t } = useTranslation();
  const messageId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  // Track hover + focus separately so we can resume the auto-dismiss
  // timer the moment either drops. WAI-ARIA APG: a toast must
  // remain visible while the user is interacting with it.
  const [hovered, setHovered] = useState(false);
  const [focusInside, setFocusInside] = useState(false);

  // Auto-dismiss timer. The `0` duration opts out — sticky toast.
  // We re-arm the timer whenever hover/focus drops, mirroring the
  // pattern the WAI-ARIA Snackbar reference implementation uses.
  useEffect(() => {
    if (!toast.durationMs || toast.durationMs <= 0) return;
    if (hovered || focusInside) return;
    if (toast.undoInFlight) return;
    const handle = window.setTimeout(() => {
      onDismiss(toast.id);
    }, toast.durationMs);
    return () => window.clearTimeout(handle);
  }, [
    toast.durationMs,
    toast.id,
    hovered,
    focusInside,
    toast.undoInFlight,
    onDismiss,
  ]);

  // Escape dismisses iff focus is inside this toast — keyboard
  // users have a way out without hunting for the × button.
  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === 'Escape' && !toast.undoInFlight) {
        event.preventDefault();
        onDismiss(toast.id);
      }
    },
    [toast.id, toast.undoInFlight, onDismiss],
  );

  const handleUndoClick = useCallback(async () => {
    if (!toast.undo || toast.undoInFlight) return;
    setUndoInFlight(toast.id, true);
    try {
      await toast.undo.action();
      // Success → just dismiss. The caller is responsible for any
      // follow-up "Undo successful" announcement.
      onDismiss(toast.id);
    } catch (err) {
      // Don't dismiss on failure — leave the toast up so the user
      // can try again. Reset the in-flight flag so the button is
      // clickable again.
      setUndoInFlight(toast.id, false);
      // eslint-disable-next-line no-console
      console.warn('toast undo action failed', err);
    }
  }, [toast.id, toast.undo, toast.undoInFlight, setUndoInFlight, onDismiss]);

  return (
    <div
      ref={rootRef}
      role="status"
      aria-live="polite"
      aria-atomic="true"
      aria-labelledby={messageId}
      className="toast"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onFocus={() => setFocusInside(true)}
      onBlur={(e) => {
        // Only flip back to "not focused" when focus genuinely
        // leaves the toast — not when it moves between the Undo and
        // dismiss buttons.
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
          setFocusInside(false);
        }
      }}
      onKeyDown={handleKeyDown}
    >
      <span id={messageId} className="toast__message">
        {toast.message}
      </span>
      {toast.undo && (
        <button
          type="button"
          className="toast__action"
          onClick={handleUndoClick}
          aria-disabled={toast.undoInFlight || undefined}
        >
          {toast.undoInFlight
            ? t('toast.undoInFlight')
            : (toast.undo.label ?? t('toast.undoLabel'))}
        </button>
      )}
      <button
        type="button"
        className="toast__dismiss"
        onClick={() => onDismiss(toast.id)}
        aria-label={t('toast.dismissLabel')}
      >
        {/* Visual × glyph; the aria-label carries the real text. */}
        <span aria-hidden="true">×</span>
      </button>
    </div>
  );
}
