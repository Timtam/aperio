import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { ContactList } from '../api/types';

/**
 * What the contact editor says when a save landed in part (decision 273): a
 * contact or a group that stands without one of its steps is said on screen
 * with only Close — saving again would create it twice — and a contact a
 * provider kept without its photo says so instead of "created".
 */

const { invokeMock, onFile, invalidateData } = vi.hoisted(() => {
  const onFile: { createFails: unknown; hasPhoto: boolean; deleteFails: unknown } = {
    createFails: null,
    hasPhoto: true,
    deleteFails: null,
  };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
    if (command === 'delete_contact' && onFile.deleteFails != null) {
      return Promise.reject(onFile.deleteFails);
    }
    if (command === 'create_contact') {
      if (onFile.createFails != null) return Promise.reject(onFile.createFails);
      const request = (payload as { request: Record<string, unknown> }).request;
      return Promise.resolve({ ...request, id: 'c-1', has_photo: onFile.hasPhoto });
    }
    return Promise.resolve(null);
  });
  return { invokeMock, onFile, invalidateData: vi.fn() };
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const BOOK = { id: 'book-1', name: 'Freunde', read_only: false } as unknown as ContactList;
// One object for every render: a fresh list each time re-runs the editor's
// effects without end.
const STORE = { contactLists: [BOOK] };
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => ({ invalidateData }) }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
  invalidateData.mockClear();
  onFile.createFails = null;
  onFile.hasPhoto = true;
  onFile.deleteFails = null;
});

async function createContact(
  onClose: () => void,
  withPhoto = false,
  beforeCreate: () => void = () => {},
) {
  const { ContactDialog } = await import('./ContactDialog');
  render(<ContactDialog isOpen onClose={onClose} contact={null} defaultListId="book-1" />);
  fireEvent.change(screen.getByLabelText('Vorname'), { target: { value: 'Anna' } });
  if (withPhoto) {
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File([new Uint8Array([0xff, 0xd8, 0xff])], 'anna.jpg', {
      type: 'image/jpeg',
    });
    fireEvent.change(input, { target: { files: [file] } });
    await screen.findByRole('button', { name: 'Foto ersetzen' });
  }
  beforeCreate();
  fireEvent.click(screen.getByRole('button', { name: 'Anlegen' }));
}

describe('ContactDialog → a create that landed in part', () => {
  it('only closes once the contact stands without one of its steps', async () => {
    onFile.createFails = { code: 'network', message: 'partly-saved: members' };
    const onClose = vi.fn();
    await createContact(onClose);
    const note = await screen.findByText(/nur zum Teil gespeichert/);
    await waitFor(() => expect(document.activeElement).toBe(note));
    expect(screen.queryByRole('button', { name: 'Anlegen' })).toBeNull();
    expect(invalidateData).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Schließen' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('says a contact the provider kept without its photo', async () => {
    onFile.hasPhoto = false;
    const onClose = vi.fn();
    await createContact(onClose, true);
    const note = await screen.findByText(
      'Kontakt Anna angelegt, aber ohne Foto: Das Foto ließ sich nicht speichern. Füge es später noch einmal hinzu.',
    );
    await waitFor(() => expect(document.activeElement).toBe(note));
    expect(onClose).not.toHaveBeenCalled();
    expect(invalidateData).toHaveBeenCalled();
  });

  it('creates a group without a photo, and closes as created', async () => {
    // A group keeps no photo (Google stores none on a group): one picked
    // before the box was ticked is not sent, and no "without its photo"
    // notice says a false failure. Mirrors the phone.
    onFile.hasPhoto = false;
    const onClose = vi.fn();
    await createContact(onClose, true, () => {
      fireEvent.click(screen.getByRole('checkbox', { name: 'Verteilerliste (Gruppe)' }));
      // Nor is one offered for a group (review round 17 of #128): the
      // editor showed a photo that was then dropped without a word.
      expect(screen.queryByRole('button', { name: 'Foto ersetzen' })).toBeNull();
      expect(document.querySelector('input[type="file"]')).toBeNull();
    });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    const sent = invokeMock.mock.calls.find((c) => c[0] === 'create_contact');
    expect((sent?.[1] as { request: { photo: unknown } }).request.photo).toBeNull();
  });

  it('writes no photo for a contact saved as a group', async () => {
    const person = {
      id: 'c-1',
      list_id: 'book-1',
      display_name: 'Anna',
      given_name: 'Anna',
      family_name: null,
      name_prefix: null,
      name_suffix: null,
      organization: null,
      emails: [],
      phone_numbers: [],
      urls: [],
      birthday: null,
      anniversary: null,
      job_title: null,
      department: null,
      notes: null,
      members: null,
      has_photo: false,
      addresses: [],
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      etag: null,
    };
    const onClose = vi.fn();
    const { ContactDialog } = await import('./ContactDialog');
    render(<ContactDialog isOpen onClose={onClose} contact={person as never} />);
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, {
      target: { files: [new File([new Uint8Array([0xff, 0xd8, 0xff])], 'anna.jpg', { type: 'image/jpeg' })] },
    });
    await screen.findByRole('button', { name: 'Foto ersetzen' });
    fireEvent.click(screen.getByRole('checkbox', { name: 'Verteilerliste (Gruppe)' }));
    fireEvent.click(screen.getByRole('button', { name: 'Speichern' }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(invokeMock.mock.calls.filter((c) => c[0] === 'update_contact')).toHaveLength(1);
    expect(invokeMock.mock.calls.filter((c) => c[0] === 'set_contact_photo')).toHaveLength(0);
  });

  it('closes as before when the photo stands too', async () => {
    const onClose = vi.fn();
    await createContact(onClose, true);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });
});

/**
 * A delete that failed may have landed — its answer was lost — so the views
 * read again (decision 277), as an event's or a task's delete does; the
 * dialog stays to say it.
 */
describe('ContactDialog → a delete that failed', () => {
  it('says it and reads the views again', async () => {
    onFile.deleteFails = { code: 'network', message: 'connection reset' };
    const contact = {
      id: 'c-1',
      list_id: 'book-1',
      display_name: 'Anna',
      given_name: 'Anna',
      family_name: null,
      name_prefix: null,
      name_suffix: null,
      organization: null,
      emails: [],
      phone_numbers: [],
      urls: [],
      birthday: null,
      anniversary: null,
      job_title: null,
      department: null,
      notes: null,
      members: null,
      has_photo: false,
      addresses: [],
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      etag: null,
    };
    const onClose = vi.fn();
    const { ContactDialog } = await import('./ContactDialog');
    render(<ContactDialog isOpen onClose={onClose} contact={contact as never} />);
    fireEvent.click(screen.getByRole('button', { name: 'Löschen' }));
    const confirm = await screen.findAllByRole('button', { name: 'Löschen' });
    fireEvent.click(confirm[confirm.length - 1]);
    await waitFor(() => expect(invalidateData).toHaveBeenCalledTimes(1));
    expect(invokeMock.mock.calls.filter((c) => c[0] === 'delete_contact')).toHaveLength(1);
    expect(onClose).not.toHaveBeenCalled();
    // Said where the dialog shows its errors.
    expect(await screen.findByText(/connection reset/)).toBeTruthy();
  });
});
