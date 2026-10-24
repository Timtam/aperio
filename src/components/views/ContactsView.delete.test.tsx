import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import { writeErrorMessage } from '@aperio/shared';

import type { Contact, ContactList } from '../../api/types';
import i18n from '../../i18n';

/**
 * The Contacts view's Delete key, after a delete that failed, reads the list
 * and any search again (decision 277), as an event's or a task's delete does:
 * a delete whose answer was lost may have landed, and the row would stay on
 * screen until the next contacts sync.
 */

const { invokeMock, invalidateData, announced } = vi.hoisted(() => ({
  invokeMock: vi.fn((command: string) =>
    command === 'delete_contact'
      ? Promise.reject({ code: 'network', message: 'connection reset' })
      : Promise.resolve(null),
  ),
  invalidateData: vi.fn(),
  announced: [] as string[],
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));
vi.mock('../../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => {
    announced.push(message);
  },
}));

const BOOK = { id: 'book-1', name: 'Freunde', read_only: false } as unknown as ContactList;
const ANNA = {
  id: 'c-1',
  list_id: 'book-1',
  display_name: 'Anna',
  given_name: 'Anna',
  family_name: null,
  organization: null,
  emails: [],
  phone_numbers: [],
  urls: [],
  members: null,
  has_photo: false,
  addresses: [],
} as unknown as Contact;
const CONTACTS = [ANNA];
const BOOKS = new Map([[BOOK.id, BOOK]]);
const STORE = { contactLists: [BOOK], selectedContactListIds: new Set([BOOK.id]) };
vi.mock('../../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../../state/useContacts', () => ({
  useContacts: () => ({ contacts: CONTACTS, loading: false, contactListById: BOOKS }),
}));
vi.mock('../../state/useContactSync', () => ({
  useContactSync: () => ({ status: null, triggering: false, triggerSync: () => {} }),
}));
vi.mock('../../state/dialogStateContext', () => ({
  useDialogState: () => ({ openContactDialog: () => {}, invalidateData }),
}));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  invalidateData.mockClear();
  announced.length = 0;
});

describe('ContactsView → the Delete key after a delete that failed (277)', () => {
  it('says the failure and reads the list again', async () => {
    const { ContactsView } = await import('./ContactsView');
    render(<ContactsView />);
    const list = await screen.findByRole('listbox');
    await screen.findByText(/Anna/);
    fireEvent.keyDown(list, { key: 'Delete' });
    const confirm = await screen.findAllByRole('button', { name: /^löschen$|^delete$/i });
    fireEvent.click(confirm[confirm.length - 1]);
    await waitFor(() => expect(invalidateData).toHaveBeenCalledTimes(1));
    expect(invokeMock.mock.calls.filter((call) => call[0] === 'delete_contact')).toHaveLength(1);
    const t = (key: string, values?: Record<string, unknown>): string =>
      i18n.t(key, values as never) as unknown as string;
    expect(announced).toEqual([
      writeErrorMessage({ code: 'network', message: 'connection reset' }, t),
    ]);
  });
});
