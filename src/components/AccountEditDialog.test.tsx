import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

import type { Account } from '../api/types';

const invokeMock = vi.hoisted(() =>
  vi.fn((cmd: string) => {
    if (cmd === 'account_form_spec') {
      return Promise.resolve({
        plugin_id: 'aperio.caldav',
        fields: [
          {
            key: 'server_url',
            kind: 'url',
            label: 'Server-URL',
            hint: null,
            required: true,
            default_bool: null,
            default_text: null,
            options: [],
            device_local: false,
          },
          {
            key: 'username',
            kind: 'text',
            label: 'Benutzername',
            hint: null,
            required: true,
            default_bool: null,
            default_text: null,
            options: [],
            device_local: false,
          },
          {
            key: 'secret',
            kind: 'secret',
            label: 'Passwort',
            hint: null,
            required: true,
            default_bool: null,
            default_text: null,
            options: [],
            device_local: false,
          },
        ],
        actions: [],
        oauth: null,
        owns_containers: true,
        supports_credential_test: true,
      });
    }
    if (cmd === 'get_user_pref') return Promise.resolve(null);
    return Promise.resolve(null);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

import { AccountEditDialog } from './AccountEditDialog';

const ACCOUNT: Account = {
  id: 'acc-1',
  display_name: 'Mein CalDAV',
  adapter_kind: 'caldav',
  config_json: '{"server_url":"https://dav.example.org","username":"toni"}',
} as unknown as Account;

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

describe('AccountEditDialog', () => {
  it('opens and renders the prefilled caldav form', async () => {
    render(
      <AccountEditDialog
        isOpen
        account={ACCOUNT}
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    expect(screen.getByRole('dialog')).toBeTruthy();
    await waitFor(() => {
      expect(screen.getByLabelText('Server-URL')).toBeTruthy();
    });
    expect(
      (screen.getByLabelText('Server-URL') as HTMLInputElement).value,
    ).toBe('https://dav.example.org');
    expect(
      (screen.getByLabelText('Benutzername') as HTMLInputElement).value,
    ).toBe('toni');
    // The schema is fetched in the UI language — without it the labels
    // come back in the plugin's English fallback while the add form is
    // localized.
    expect(invokeMock).toHaveBeenCalledWith(
      'account_form_spec',
      expect.objectContaining({ adapterKind: 'caldav', lang: 'de' }),
    );
  });
});
