import { createContext, useContext } from 'react';

import type { CalendarStoreState } from './CalendarStore';

/**
 * Calendar/task selection store context + consumer hook. Split out of
 * `CalendarStoreProvider` so that component file exports only its
 * component (Fast Refresh). The state type + the provider
 * implementation live alongside the component.
 */
export const CalendarStoreContext = createContext<CalendarStoreState | null>(
  null,
);

export function useCalendarStore(): CalendarStoreState {
  const ctx = useContext(CalendarStoreContext);
  if (!ctx) {
    throw new Error(
      'useCalendarStore must be used inside <CalendarStoreProvider>',
    );
  }
  return ctx;
}
