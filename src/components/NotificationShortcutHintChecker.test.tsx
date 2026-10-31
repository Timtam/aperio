import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { NotificationShortcutHintChecker } from './NotificationShortcutHintChecker';

/**
 * When the one-time hint opens: a reminder came due while Windows would
 * hide it, and only once Aperio's window has focus, no other dialog is open
 * and setting up is still needed. Once per app start.
 */

const h = vi.hoisted(() => {
  const order: string[] = [];
  const dialog = {
    mode: { kind: 'none' } as { kind: string },
    openNotificationShortcutHint: vi.fn(() => {
      order.push('open');
    }),
  };
  const backend = {
    pending: false,
    status: { kind: 'not_set_up', needs_set_up: true } as { kind: string; needs_set_up: boolean },
    shownFails: false,
  };
  const invokeMock = vi.fn((cmd: string) => {
    switch (cmd) {
      case 'notification_shortcut_hint_pending':
        return Promise.resolve(backend.pending);
      case 'notification_shortcut_status':
        return Promise.resolve(backend.status);
      case 'notification_shortcut_hint_shown':
        order.push('shown');
        return backend.shownFails ? Promise.reject(new Error('gone')) : Promise.resolve();
      default:
        return Promise.resolve(null);
    }
  });
  const events: { hint: (() => void) | null; focus: ((e: { payload: boolean }) => void) | null } = {
    hint: null,
    focus: null,
  };
  const win = { focused: true };
  return { order, dialog, backend, invokeMock, events, win };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: h.invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (name: string, handler: () => void) => {
    if (name === 'notification-shortcut-hint') h.events.hint = handler;
    return Promise.resolve(() => {});
  },
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isFocused: () => Promise.resolve(h.win.focused),
    onFocusChanged: (handler: (e: { payload: boolean }) => void) => {
      h.events.focus = handler;
      return Promise.resolve(() => {});
    },
  }),
}));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => h.dialog }));

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 20)));

async function mount() {
  const view = render(<NotificationShortcutHintChecker />);
  await settle();
  return view;
}

async function fire() {
  await act(async () => h.events.hint?.());
  await settle();
}

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  h.order.length = 0;
  h.dialog.mode = { kind: 'none' };
  h.dialog.openNotificationShortcutHint.mockClear();
  h.invokeMock.mockClear();
  h.backend.pending = false;
  h.backend.status = { kind: 'not_set_up', needs_set_up: true };
  h.backend.shownFails = false;
  h.events.hint = null;
  h.events.focus = null;
  h.win.focused = true;
});

const opened = () => h.dialog.openNotificationShortcutHint.mock.calls.length;

describe('the notification shortcut hint', () => {
  it('opens once a reminder was hidden, marks itself shown first, and never twice', async () => {
    await mount();
    expect(opened()).toBe(0);
    await fire();
    expect(h.order).toEqual(['shown', 'open']);
    await fire();
    expect(opened()).toBe(1);
  });

  it('opens for a hint raised before it listened', async () => {
    h.backend.pending = true;
    await mount();
    expect(opened()).toBe(1);
  });

  it('waits for Aperio to have focus', async () => {
    h.win.focused = false;
    await mount();
    await fire();
    expect(opened()).toBe(0);
    await act(async () => h.events.focus?.({ payload: true }));
    await settle();
    expect(opened()).toBe(1);
  });

  it('waits for an open dialog to close', async () => {
    h.dialog.mode = { kind: 'editEvent' };
    const view = await mount();
    await fire();
    expect(opened()).toBe(0);
    h.dialog.mode = { kind: 'none' };
    view.rerender(<NotificationShortcutHintChecker />);
    await settle();
    expect(opened()).toBe(1);
  });

  it('stays away when the shortcut was set up meanwhile', async () => {
    await mount();
    h.backend.status = { kind: 'set_up', needs_set_up: false };
    await fire();
    expect(opened()).toBe(0);
    expect(h.order).toEqual([]);
  });

  it('still opens when marking it shown fails', async () => {
    h.backend.shownFails = true;
    await mount();
    await fire();
    expect(opened()).toBe(1);
  });
});
