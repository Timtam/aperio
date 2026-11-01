import { act, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The switch "Kalenderwochen anzeigen und ansagen" rides the synced pref
 * `view.weekNumbers`: on until the stored value says otherwise, written back
 * the way the phone reads it, and read again when a sync round brings a
 * change from another device.
 */

const h = vi.hoisted(() => ({
  prefs: new Map<string, string>(),
  written: [] as [string, string][],
  prefsChanged: [] as ((event: { payload: string[] }) => void)[],
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: (name: string, handler: (event: { payload: string[] }) => void) => {
    if (name === 'user-prefs-changed') h.prefsChanged.push(handler);
    return Promise.resolve(() => {});
  },
}));

vi.mock('../api/client', () => ({
  getUserPref: (key: string) => Promise.resolve(h.prefs.get(key) ?? null),
  setUserPref: (key: string, value: string) => {
    h.written.push([key, value]);
    return Promise.resolve();
  },
}));

import { ViewStateProvider } from './ViewState';
import { useViewState } from './viewStateContext';

function Probe() {
  const { weekNumbers, setWeekNumbers } = useViewState();
  return (
    <button type="button" onClick={() => setWeekNumbers(!weekNumbers)}>
      {weekNumbers ? 'on' : 'off'}
    </button>
  );
}

function renderProbe() {
  render(
    <ViewStateProvider>
      <Probe />
    </ViewStateProvider>,
  );
  return screen.getByRole('button');
}

beforeEach(() => {
  localStorage.clear();
  h.prefs.clear();
  h.written.length = 0;
  h.prefsChanged.length = 0;
});

describe('the calendar week switch', () => {
  it('is on when nothing is stored', async () => {
    const button = renderProbe();
    await act(async () => {});
    expect(button.textContent).toBe('on');
  });

  it('follows the stored value', async () => {
    h.prefs.set('view.weekNumbers', 'false');
    const button = renderProbe();
    await waitFor(() => expect(button.textContent).toBe('off'));
  });

  it('writes the synced pref', async () => {
    const button = renderProbe();
    await act(async () => {});
    act(() => button.click());
    expect(button.textContent).toBe('off');
    expect(h.written).toContainEqual(['view.weekNumbers', 'false']);
  });

  it('follows a change a sync round brings, without a restart', async () => {
    const button = renderProbe();
    await act(async () => {});
    expect(button.textContent).toBe('on');
    // Switched off on the phone; the round writes it here and says so.
    h.prefs.set('view.weekNumbers', 'false');
    act(() => h.prefsChanged.forEach((handler) => handler({ payload: ['view.weekNumbers'] })));
    await waitFor(() => expect(button.textContent).toBe('off'));
  });
});
