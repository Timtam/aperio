import { act, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The switch "Kalenderwochen anzeigen und ansagen" rides the synced pref
 * `view.weekNumbers`: on until the stored value says otherwise, and written
 * back the way the phone reads it.
 */

const h = vi.hoisted(() => ({
  prefs: new Map<string, string>(),
  written: [] as [string, string][],
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
});
