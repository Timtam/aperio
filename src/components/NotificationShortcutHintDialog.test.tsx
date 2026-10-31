import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AnnouncerProvider } from '../a11y/Announcer';
import { NotificationShortcutHintDialog } from './NotificationShortcutHintDialog';

/**
 * The one-time hint: opens on its explanation, sets up and closes, keeps a
 * failure in view, and cannot be closed while setting up runs (its success
 * would then close whatever dialog came next).
 */

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

let finishCreate: (value: unknown) => void = () => {};

function backend(create: 'ok' | 'fail' | 'hold') {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === 'notification_shortcut_status') {
      return Promise.resolve({ kind: 'not_set_up', needs_set_up: true });
    }
    if (cmd === 'notification_shortcut_create') {
      if (create === 'ok') return Promise.resolve({ kind: 'set_up', needs_set_up: false });
      if (create === 'fail') return Promise.reject({ code: 'internal', message: 'verweigert' });
      return new Promise((resolve) => {
        finishCreate = resolve;
      });
    }
    return Promise.resolve(null);
  });
}

const onClose = vi.fn();

function show() {
  render(
    <AnnouncerProvider>
      <NotificationShortcutHintDialog isOpen onClose={onClose} />
    </AnnouncerProvider>,
  );
}

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 200)));
const creates = () =>
  invokeMock.mock.calls.filter(([cmd]) => cmd === 'notification_shortcut_create').length;

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  invokeMock.mockReset();
  onClose.mockReset();
});
afterEach(() => {
  document.body.innerHTML = '';
});

describe('the notification shortcut hint dialog', () => {
  it('opens on its explanation', async () => {
    backend('ok');
    show();
    await settle();
    expect(document.activeElement?.textContent).toMatch(
      /^Eine Erinnerung ist fällig geworden, aber Windows hat sie nicht angezeigt/,
    );
  });

  it('sets up, says so, and closes', async () => {
    backend('ok');
    show();
    fireEvent.click(screen.getByRole('button', { name: 'Benachrichtigungen einrichten' }));
    await settle();
    expect(creates()).toBe(1);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('announcer-polite').textContent).toBe(
      'Benachrichtigungen eingerichtet. Windows kann die Erinnerungen von Aperio jetzt anzeigen.',
    );
  });

  it('keeps a failure in view and stays open', async () => {
    backend('fail');
    show();
    fireEvent.click(screen.getByRole('button', { name: 'Benachrichtigungen einrichten' }));
    await screen.findByText('Der Startmenü-Eintrag konnte nicht geändert werden: verweigert');
    expect(onClose).not.toHaveBeenCalled();
  });

  it('cannot be closed while setting up runs', async () => {
    backend('hold');
    show();
    fireEvent.click(screen.getByRole('button', { name: 'Benachrichtigungen einrichten' }));
    await settle();
    fireEvent.click(screen.getByRole('button', { name: 'Nicht jetzt' }));
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => finishCreate({ kind: 'set_up', needs_set_up: false }));
    await settle();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('"Not now" closes without setting anything up', async () => {
    backend('ok');
    show();
    await settle();
    fireEvent.click(screen.getByRole('button', { name: 'Nicht jetzt' }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(creates()).toBe(0);
  });
});
