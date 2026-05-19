import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

/**
 * Global announcer for screen readers.
 *
 * Renders two visually-hidden ARIA live regions — one `polite`, one
 * `assertive` — and exposes a single [`announce`] function that picks the
 * right region.
 *
 * **Why two regions:**
 * - `polite` for routine status updates ("Calendar created", "Wednesday,
 *   14 May, 2 events"). The screen reader waits until the user pauses
 *   before reading the message.
 * - `assertive` for conflicts and errors the user must hear right now
 *   (sync conflict dialog, write failure). Interrupts the current
 *   utterance.
 *
 * **The blank-out trick.** Screen readers ignore live-region updates when
 * the new content is identical to the previous content. To re-announce
 * the same string (e.g. tapping "Today" twice), we clear the region first
 * and only then write the new text — see [`announce`] below.
 */
export type Urgency = 'polite' | 'assertive';

type AnnouncerContextValue = {
  announce: (message: string, urgency?: Urgency) => void;
};

const AnnouncerContext = createContext<AnnouncerContextValue | null>(null);

export function AnnouncerProvider({ children }: { children: ReactNode }) {
  const [polite, setPolite] = useState('');
  const [assertive, setAssertive] = useState('');
  const clearTimer = useRef<number | null>(null);

  // Cancel pending clear when the provider unmounts.
  useEffect(
    () => () => {
      if (clearTimer.current !== null) {
        window.clearTimeout(clearTimer.current);
      }
    },
    [],
  );

  const announce = useCallback((message: string, urgency: Urgency = 'polite') => {
    const setter = urgency === 'polite' ? setPolite : setAssertive;
    // Blank, then on the next frame write the message. The double-RAF is
    // there because React batches state updates within a single tick and
    // we need the DOM to actually flip to "" before flipping back.
    setter('');
    if (clearTimer.current !== null) {
      window.clearTimeout(clearTimer.current);
    }
    requestAnimationFrame(() => {
      requestAnimationFrame(() => setter(message));
    });
    // Auto-clear after 30 s so the region does not hold stale text the
    // next time someone uses a screen-reader "read all" command.
    clearTimer.current = window.setTimeout(() => setter(''), 30_000);
  }, []);

  const value = useMemo(() => ({ announce }), [announce]);

  return (
    <AnnouncerContext.Provider value={value}>
      {children}
      <div
        aria-live="polite"
        aria-atomic="true"
        role="status"
        className="sr-only"
        data-testid="announcer-polite"
      >
        {polite}
      </div>
      <div
        aria-live="assertive"
        aria-atomic="true"
        role="alert"
        className="sr-only"
        data-testid="announcer-assertive"
      >
        {assertive}
      </div>
    </AnnouncerContext.Provider>
  );
}

/**
 * Returns the `announce` function from the nearest [`AnnouncerProvider`].
 *
 * Throws if no provider is mounted — that always indicates a wiring bug,
 * never a runtime condition we can recover from.
 */
export function useAnnouncer(): AnnouncerContextValue['announce'] {
  const ctx = useContext(AnnouncerContext);
  if (!ctx) {
    throw new Error('useAnnouncer must be used inside <AnnouncerProvider>');
  }
  return ctx.announce;
}
