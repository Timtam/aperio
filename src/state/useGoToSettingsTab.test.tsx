import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { SettingsNavContext } from './settingsNavContext';
import { useGoToSettingsTab } from './useGoToSettingsTab';

/**
 * The jump from Sync or Kontakte to Allgemein, where the refresh interval
 * now lives: inside Settings in place (a second dialog frame would unmount
 * the pressed button and drop NVDA out of the dialog), elsewhere by opening
 * Settings on that tab.
 */

const h = vi.hoisted(() => ({ openSettings: vi.fn() }));
vi.mock('./dialogStateContext', () => ({ useDialogState: () => ({ openSettings: h.openSettings }) }));

describe('useGoToSettingsTab', () => {
  it('switches the tab in place inside Settings', () => {
    const goToTab = vi.fn();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <SettingsNavContext.Provider value={{ goToTab }}>{children}</SettingsNavContext.Provider>
    );
    const { result } = renderHook(() => useGoToSettingsTab('general'), { wrapper });
    result.current();
    expect(goToTab).toHaveBeenCalledWith('general');
    expect(h.openSettings).not.toHaveBeenCalled();
  });

  it('opens Settings on that tab from anywhere else', () => {
    h.openSettings.mockClear();
    const { result } = renderHook(() => useGoToSettingsTab('general'));
    result.current();
    expect(h.openSettings).toHaveBeenCalledWith('general');
  });
});
