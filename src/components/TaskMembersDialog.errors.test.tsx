import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

/**
 * The members dialog reads the list's shares and writes them. A read that a
 * busy provider turned away says so in words, and never that nothing was
 * changed: nothing was asked to change (decision 273).
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) =>
    command === 'task_list_shares'
      ? Promise.reject({ code: 'forbidden', message: 'provider-busy: http-429' })
      : Promise.resolve([]),
  ),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

describe('TaskMembersDialog → a read that failed', () => {
  it('says a busy provider neutrally', async () => {
    const { TaskMembersDialog } = await import('./TaskMembersDialog');
    render(<TaskMembersDialog isOpen onClose={() => {}} listId="L1" listName="Haushalt" />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(
      'Der Anbieter ist gerade ausgelastet. Versuche es später noch einmal.',
    );
    expect(alert.textContent).not.toContain('Es wurde nichts geändert');
  });
});
