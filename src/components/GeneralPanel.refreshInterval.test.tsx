import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { GeneralPanel } from './GeneralPanel';

/**
 * The one refresh interval under Allgemein (Toni, 2026-11-09): settable with
 * or without a sync target, its hint read with the field, and a change said
 * in words with the right plural.
 */

const h = vi.hoisted(() => {
  const backend = { interval: 5, configured: false };
  const invokeMock = vi.fn((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'get_sync_status':
        return Promise.resolve({ configured: backend.configured, interval_minutes: backend.interval });
      case 'set_sync_interval':
        backend.interval = args?.minutes as number;
        return Promise.resolve(backend.interval);
      case 'tray_available':
      case 'autostart_is_enabled':
        return Promise.resolve(false);
      default:
        return Promise.resolve(null);
    }
  });
  const viewState = new Proxy(
    { weekStartsOn: 1, weekNumbers: false, timeStepMinutes: 15 },
    {
      get: (target, key: string) =>
        key in target ? target[key as keyof typeof target] : key.startsWith('set') ? () => {} : false,
    },
  );
  const announce = vi.fn();
  return { backend, invokeMock, viewState, announce };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: h.invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({ listen: () => Promise.resolve(() => {}) }));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => h.viewState }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));
vi.mock('./NotificationShortcutSection', () => ({ NotificationShortcutSection: () => null }));

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 20)));

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  h.backend.interval = 5;
  h.backend.configured = false;
  h.invokeMock.mockClear();
  h.announce.mockClear();
});

const field = () =>
  screen.getByRole('combobox', { name: /aktualisierungs-intervall|refresh interval/i }) as HTMLSelectElement;

describe('the refresh interval under Allgemein', () => {
  it('can be set without a sync target, and its hint is read with it', async () => {
    render(<GeneralPanel />);
    await settle();
    const select = field();
    expect(select.disabled).toBe(false);
    expect(select.value).toBe('5');
    const hint = document.getElementById(select.getAttribute('aria-describedby') ?? '');
    expect(hint?.textContent).toMatch(/Kalender, Aufgabenlisten und Kontakte|calendars, task lists and contacts/);
  });

  it('saves a change and says it, one minute in the singular', async () => {
    render(<GeneralPanel />);
    await settle();
    fireEvent.change(field(), { target: { value: '1' } });
    await settle();
    const set = h.invokeMock.mock.calls.find((c) => c[0] === 'set_sync_interval');
    expect(set?.[1]).toEqual({ minutes: 1 });
    expect(h.announce).toHaveBeenCalledWith(expect.stringMatching(/1 Minute gesetzt|1 minute\./));
  });

  it('shows a stored value that is no preset as it is', async () => {
    h.backend.interval = 7;
    render(<GeneralPanel />);
    await settle();
    expect(field().value).toBe('7');
  });
});
