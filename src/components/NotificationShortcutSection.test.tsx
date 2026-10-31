import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AnnouncerProvider } from '../a11y/Announcer';
import type { NotificationShortcutStatus } from '../api/client';
import { NotificationShortcutSection } from './NotificationShortcutSection';

/**
 * Settings, General, Windows notifications: the Start menu shortcut Windows
 * needs before it shows Aperio's reminders. Through the real announcer,
 * since a press whose button keeps focus has to say what it did.
 */

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const NOT_SET_UP: NotificationShortcutStatus = { kind: 'not_set_up', needs_set_up: true };
const SET_UP: NotificationShortcutStatus = { kind: 'set_up', needs_set_up: false };

/** Answers per command; a function answers each call. */
function backend(answers: Record<string, unknown>) {
  invokeMock.mockImplementation((cmd: string) => {
    const answer = answers[cmd];
    if (typeof answer === 'function') return (answer as () => unknown)();
    return Promise.resolve(answer);
  });
}

function show(variant: 'settings' | 'wizard' = 'settings') {
  return render(
    <AnnouncerProvider>
      <NotificationShortcutSection variant={variant} />
    </AnnouncerProvider>,
  );
}

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 200)));
const calls = (cmd: string) => invokeMock.mock.calls.filter(([c]) => c === cmd).length;

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  invokeMock.mockReset();
});
afterEach(() => {
  document.body.innerHTML = '';
});

describe('the Windows notifications section', () => {
  it('shows nothing off Windows', async () => {
    backend({ notification_shortcut_status: { kind: 'unsupported', needs_set_up: false } });
    show();
    await settle();
    expect(screen.queryByRole('heading', { name: 'Windows-Benachrichtigungen' })).toBeNull();
  });

  it('says it is not set up and keeps Remove focusable but unavailable', async () => {
    backend({ notification_shortcut_status: NOT_SET_UP });
    show();
    await screen.findByText(/Nicht eingerichtet\. Windows zeigt die Erinnerungen/);
    const remove = screen.getByRole('button', { name: 'Startmenü-Eintrag entfernen' });
    expect(remove.getAttribute('aria-disabled')).toBe('true');
    expect(remove.hasAttribute('disabled')).toBe(false);
    expect(
      screen.getByRole('button', { name: 'Benachrichtigungen einrichten' }).getAttribute(
        'aria-disabled',
      ),
    ).toBe('false');
  });

  it('sets up once however often it is pressed, keeps focus and says so once', async () => {
    let finish: (status: NotificationShortcutStatus) => void = () => {};
    backend({
      notification_shortcut_status: NOT_SET_UP,
      notification_shortcut_create: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    show();
    const setUp = await screen.findByRole('button', { name: 'Benachrichtigungen einrichten' });
    setUp.focus();
    fireEvent.click(setUp);
    fireEvent.click(setUp);
    expect(calls('notification_shortcut_create')).toBe(1);
    await act(async () => finish(SET_UP));
    await settle();
    expect(document.activeElement).toBe(setUp);
    expect(screen.getByTestId('announcer-polite').textContent).toBe(
      'Benachrichtigungen eingerichtet. Windows kann die Erinnerungen von Aperio jetzt anzeigen.',
    );
    expect(screen.getByText(/^Eingerichtet\./)).toBeTruthy();
    expect(setUp.getAttribute('aria-disabled')).toBe('true');
  });

  it('shows a failure as one alert, says nothing else, and reads the status again', async () => {
    backend({
      notification_shortcut_status: NOT_SET_UP,
      notification_shortcut_create: () =>
        Promise.reject({ code: 'internal', message: 'Zugriff verweigert' }),
    });
    show();
    fireEvent.click(await screen.findByRole('button', { name: 'Benachrichtigungen einrichten' }));
    const line = await screen.findByText(
      'Der Startmenü-Eintrag konnte nicht geändert werden: Zugriff verweigert',
    );
    expect(line.getAttribute('role')).toBe('alert');
    await settle();
    expect(screen.getByTestId('announcer-polite').textContent).toBe('');
    await waitFor(() => expect(calls('notification_shortcut_status')).toBe(2));
  });

  it('does not offer to remove an entry Aperio did not make', async () => {
    backend({ notification_shortcut_status: { kind: 'foreign', needs_set_up: true } });
    show();
    const remove = await screen.findByRole('button', { name: 'Startmenü-Eintrag entfernen' });
    expect(remove.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(remove);
    expect(calls('notification_shortcut_remove')).toBe(0);
  });

  it('names the other copy and offers both buttons', async () => {
    backend({
      notification_shortcut_status: {
        kind: 'points_elsewhere',
        target: 'E:\\Aperio\\aperio.exe',
        needs_set_up: false,
      },
    });
    show();
    await screen.findByText(/E:\\Aperio\\aperio\.exe/);
    for (const name of ['Benachrichtigungen einrichten', 'Startmenü-Eintrag entfernen']) {
      expect(screen.getByRole('button', { name }).getAttribute('aria-disabled')).toBe('false');
    }
  });

  it('explains a development build and offers nothing', async () => {
    backend({ notification_shortcut_status: { kind: 'dev_build', needs_set_up: false } });
    show();
    await screen.findByText(/Windows PowerShell/);
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('says when the status could not be read, without buttons', async () => {
    backend({
      notification_shortcut_status: () =>
        Promise.reject({ code: 'internal', message: 'kein Startmenü' }),
    });
    show();
    await screen.findByText(
      'Aperio konnte den Startmenü-Eintrag nicht prüfen: kein Startmenü',
    );
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('offers only setting up in the first-launch wizard, under its heading', async () => {
    backend({ notification_shortcut_status: NOT_SET_UP });
    show('wizard');
    await screen.findByRole('button', { name: 'Benachrichtigungen einrichten' });
    expect(screen.queryByRole('button', { name: 'Startmenü-Eintrag entfernen' })).toBeNull();
    expect(screen.queryByRole('heading')).toBeNull();
  });
});
