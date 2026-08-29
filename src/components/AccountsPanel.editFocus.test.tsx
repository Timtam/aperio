import { StrictMode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';

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
            singleton_existing: false,
            owns_containers: true,
            declares_oauth: false,
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
vi.mock('../state/useRefreshErrors', () => ({
  useRefreshErrors: () => ({ errorsByAccount: new Map() }),
}));

import { AccountsPanel } from './AccountsPanel';

afterEach(() => {
  document.body.innerHTML = '';
});

/**
 * Regression: pressing "„…“ bearbeiten" in Settings → Konten opened the
 * edit dialog, but StrictMode's double-invoked Modal mount effect queued
 * a focus-restore to the trigger button — inside the (non-inert) Settings
 * dialog, that restore WON, so focus never entered the edit dialog and a
 * screen reader announced nothing at all ("es passiert gar nichts").
 */
describe('AccountsPanel edit button', () => {
  it('opens the edit dialog for a caldav account and moves focus into it', async () => {
    // StrictMode, like the dev build: the double-invoked Modal mount
    // effect is exactly what yanked focus back out of the edit dialog.
    // The async act drains the panel's mount fetches (accounts, kinds,
    // missing-credentials, the add form's schema) inside act — resolved
    // between waitFor polls they'd fire the not-wrapped-in-act warning.
    await act(async () => {
      render(
        <StrictMode>
          <AccountsPanel />
        </StrictMode>,
      );
    });
    const edit = await screen.findByRole('button', {
      name: '„Mein CalDAV“ bearbeiten',
    });
    // Let the panel's parallel mount fetches settle inside act before
    // interacting — the missing-credentials probe and the add form's own
    // schema fetch both setState when they land, and an un-awaited one
    // fires the not-wrapped-in-act warning.
    await waitFor(() => {
      expect(screen.getAllByLabelText('Server-URL').length).toBeGreaterThan(0);
    });
    // Async act: opening the dialog moves focus in a microtask cascade
    // (Modal's open effect, the listbox's blur bookkeeping) — flushed
    // inside act so no update lands outside it.
    await act(async () => {
      edit.focus();
      fireEvent.click(edit);
    });
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => {
      // The schema form arrived inside the DIALOG (the add form on the
      // panel behind it carries the same labels, so scope the query).
      expect(within(dialog).getByLabelText('Server-URL')).toBeTruthy();
    });
    // The whole point for a screen-reader user: focus must have moved
    // INTO the dialog, or opening it is indistinguishable from nothing.
    await waitFor(() => {
      expect(dialog.contains(document.activeElement)).toBe(true);
    });
    // Drain the trailing async work (device-local pref seeding etc.) so
    // no state update lands outside act after the last assertion.
    await act(async () => {});
  });
});
