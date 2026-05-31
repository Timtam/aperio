import { createContext, useContext } from 'react';

/**
 * Screen-reader announcer context + consumer hook. Split out of
 * `Announcer` so that component file exports only its provider
 * component (Fast Refresh). The live-region rendering lives there.
 */
export type Urgency = 'polite' | 'assertive';

export type AnnouncerContextValue = {
  announce: (message: string, urgency?: Urgency) => void;
};

export const AnnouncerContext = createContext<AnnouncerContextValue | null>(
  null,
);

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
