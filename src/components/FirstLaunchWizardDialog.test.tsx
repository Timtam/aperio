import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AnnouncerProvider } from '../a11y/Announcer';
import { FirstLaunchWizardDialog } from './FirstLaunchWizardDialog';

/**
 * The first-launch wizard's step on Windows notifications: counted in the
 * step indicator, focused on its heading when reached, and between the
 * language and the first account. Without it the wizard is as before.
 */

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn((cmd: string) =>
    Promise.resolve(
      cmd === 'notification_shortcut_status' ? { kind: 'not_set_up', needs_set_up: true } : null,
    ),
  ),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../intl/language', () => ({
  readLanguagePref: () => Promise.resolve('system'),
  setLanguagePref: () => Promise.resolve(),
}));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ openAccounts: () => {}, openSyncAccountsConnect: () => {} }),
}));

function show(notificationStep: boolean) {
  render(
    <AnnouncerProvider>
      <FirstLaunchWizardDialog isOpen onClose={() => {}} notificationStep={notificationStep} />
    </AnnouncerProvider>,
  );
}

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 20)));
const press = async (name: string) => {
  fireEvent.click(screen.getByRole('button', { name }));
  await settle();
};

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  invokeMock.mockClear();
});
afterEach(() => {
  document.body.innerHTML = '';
});

describe('the first-launch wizard', () => {
  it('asks about Windows notifications between the language and the first account', async () => {
    show(true);
    await settle();
    expect(screen.getByText('Schritt 1 von 4')).toBeTruthy();
    await press('Weiter');
    expect(screen.getByText('Schritt 2 von 4')).toBeTruthy();
    expect(document.activeElement?.textContent).toBe('Erinnerungen unter Windows');
    expect(screen.getByRole('button', { name: 'Benachrichtigungen einrichten' })).toBeTruthy();
    await press('Weiter');
    expect(screen.getByText('Schritt 3 von 4')).toBeTruthy();
    expect(document.activeElement?.textContent).toBe('Erstes Konto hinzufügen');
    await press('Zurück');
    expect(document.activeElement?.textContent).toBe('Erinnerungen unter Windows');
  });

  it('is three steps without it, as before', async () => {
    show(false);
    await settle();
    expect(screen.getByText('Schritt 1 von 3')).toBeTruthy();
    await press('Weiter');
    expect(screen.getByText('Schritt 2 von 3')).toBeTruthy();
    expect(document.activeElement?.textContent).toBe('Erstes Konto hinzufügen');
    expect(
      invokeMock.mock.calls.filter(([cmd]) => cmd === 'notification_shortcut_status'),
    ).toHaveLength(0);
  });
});
