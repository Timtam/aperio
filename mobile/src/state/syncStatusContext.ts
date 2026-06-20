import { createContext, useContext } from 'react';

import type { SyncStatusInfo } from './useSyncStatus';

// App-wide sync status, provided ONCE from the root `useSyncStatus` poll so any
// sub-screen can show the indicator (the desktop's status pill) without spinning
// up a second poller. The native tab bar has no slot for an extra custom
// accessible control, so the indicator lives in each main screen's header instead.
export const SyncStatusContext = createContext<SyncStatusInfo | null>(null);

export function useSyncStatusInfo(): SyncStatusInfo | null {
  return useContext(SyncStatusContext);
}
