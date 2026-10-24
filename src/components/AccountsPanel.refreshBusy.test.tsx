import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

const CALDAV_SPEC = {
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
};

const invokeMock = vi.hoisted(() =>
  vi.fn((cmd: string) => {
    switch (cmd) {
      case 'list_accounts':
        return Promise.resolve([
          {
            id: 'acc-1',
            display_name: 'Mein CalDAV',
            adapter_kind: 'caldav',
            config_json: '{"server_url":"https://dav.example.org"}',
            plugin_loaded: true,
          },
        ]);
      case 'list_adapter_kinds':
        return Promise.resolve([
          {
            kind: 'caldav',
            offered: true,
            implicit: false,
            single_instance: false,
            name: 'CalDAV',
            short_name: 'CalDAV',
            plugin_id: 'com.aperio.cal-adapter-caldav',
            owns_containers: true,
            declares_account_schema: true,
            declares_oauth: false,
            holds_data: true,
            can_sync: false,
          },
        ]);
      case 'list_accounts_missing_credentials':
        return Promise.resolve([]);
      case 'account_form_spec':
        return Promise.resolve(CALDAV_SPEC);
      case 'get_user_pref':
        return Promise.resolve(null);
      default:
        return Promise.resolve(null);
    }
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => ({
    refreshCalendars: () => Promise.resolve(),
    refreshTaskLists: () => Promise.resolve(),
    refreshContactLists: () => Promise.resolve(),
    refreshAccounts: () => Promise.resolve(),
  }),
}));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({
    openSyncAccountsConnect: () => {},
    dataVersion: 0,
  }),
}));
vi.mock('../state/useRefreshErrors', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../state/useRefreshErrors')>();
  return {
    ...actual,
    useRefreshErrors: () => ({
      errorsByAccount: new Map([
        [
          'acc-1',
          {
            account_id: 'acc-1',
            auth_suspected: false,
            no_access: false,
            cause: 'busy',
            rank: 1,
            errors: [
              {
                scope: 'events',
                container_id: 'cal-1',
                container_name: 'Arbeit',
                error: 'access denied: provider-busy: http-429',
                last_success_at: null,
                cause: 'busy',
              },
            ],
          },
        ],
      ]),
    }),
  };
});

import { AccountsPanel } from './AccountsPanel';

afterEach(() => {
  document.body.innerHTML = '';
});

/**
 * A busy provider on the accounts panel (decisions 271, 272): the account
 * says there is nothing to do and that Aperio tries again by itself, offers
 * no sign-in, and its rows say the provider is busy in words — never the
 * recorded token.
 */
describe('AccountsPanel with a busy provider', () => {
  it('says to wait, offers no reconnect, and words the row', async () => {
    await act(async () => {
      render(<AccountsPanel />);
    });
    await screen.findByText(/Aperio versucht es von selbst erneut/);
    expect(screen.queryByRole('button', { name: /Passwort|anmelden/i })).toBeNull();
    const row = await screen.findByText(/^Arbeit: Der Anbieter ist ausgelastet\./);
    expect(row.textContent).not.toMatch(/provider-busy|access denied/);
  });
});
