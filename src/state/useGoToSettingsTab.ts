import { useCallback } from 'react';

import type { SettingsTabId } from '../components/SettingsDialog';
import { useDialogState } from './dialogStateContext';
import { useSettingsNav } from './settingsNavContext';

/**
 * A button's way to another Settings tab: in place inside the Settings
 * dialog, without a second dialog frame (see `settingsNavContext.ts`), and by
 * opening Settings on that tab from anywhere else.
 */
export function useGoToSettingsTab(tab: SettingsTabId): () => void {
  const { openSettings } = useDialogState();
  const settingsNav = useSettingsNav();
  return useCallback(() => {
    if (settingsNav) settingsNav.goToTab(tab);
    else openSettings(tab);
  }, [openSettings, settingsNav, tab]);
}
