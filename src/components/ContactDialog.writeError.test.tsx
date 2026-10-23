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
  const onFile: { createFails: unknown; hasPhoto: boolean } = {
    createFails: null,
    hasPhoto: true,
  };
  const invokeMock = vi.fn((command: string, payload?: unknown) => {
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
});

async function createContact(onClose: () => void, withPhoto = false) {
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

  it('closes as before when the photo stands too', async () => {
    const onClose = vi.fn();
    await createContact(onClose, true);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });
});
