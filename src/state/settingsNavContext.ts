import { createContext, useContext } from 'react';

import type { SettingsTabId } from '../components/SettingsDialog';

/**
 * How a Settings PANEL sends the user to another Settings TAB.
 *
 * `openSettings('accounts')` is the wrong tool from inside the dialog: it
 * PUSHES a second `settings` frame, and `DialogHost` renders `SettingsDialog`
 * at the same tree position for both — so React reconciles in place, the
 * dialog never closes, `Modal`'s open-focus effect never re-runs (its deps did
 * not change), and the panel that hosted the pressed button is unmounted
 * underneath the user. Focus lands on `<body>`, outside the modal's
 * `role="application"`, which drops NVDA out of the dialog entirely. The extra
 * frame also rewrites every stacked `settings` frame's tab, so Escape no
 * longer returns to the tab the user came from.
 *
 * Switching the tab IN PLACE is what the mobile twin does (a sibling route on
 * the same stack) and it has somewhere deliberate to put focus: the
 * destination tab button, which announces its own name.
 *
 * Split out of `SettingsDialog` so that component file keeps exporting only
 * its component (Fast Refresh), the same split `dialogStateContext.ts` makes.
 */
export interface SettingsNavValue {
  /** Show `tab` in the CURRENT Settings dialog and move focus onto its tab
   *  button. No new dialog frame, no unmount of the dialog itself. */
  goToTab: (tab: SettingsTabId) => void;
}

export const SettingsNavContext = createContext<SettingsNavValue | null>(null);

/**
 * `null` outside `SettingsDialog` — panels that can also render elsewhere
 * (the first-launch wizard) must keep a fallback.
 */
export function useSettingsNav(): SettingsNavValue | null {
  return useContext(SettingsNavContext);
}
