import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { TaskListShare } from '@aperio/shared';

/**
 * A member write whose answer was lost may still have landed (review round 21
 * of #128): the dialog reads the members again, as they now stand, and puts
 * an old role back only where the change surely never landed.
 */

const state = vi.hoisted(() => ({
  reads: [] as (TaskListShare[] | Promise<TaskListShare[]>)[],
  failure: null as unknown,
}));
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) => {
    if (command === 'task_list_shares') {
      return Promise.resolve(state.reads.length > 1 ? state.reads.shift()! : state.reads[0]);
    }
    if (
      command === 'task_set_member_right' ||
      command === 'task_add_member' ||
      command === 'task_remove_member'
    ) {
      return Promise.reject(state.failure);
    }
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

const erika = (right: TaskListShare['right']): TaskListShare => ({
  user: { id: 'u1', name: 'Erika', email: null },
  right,
  pending: false,
});
const max: TaskListShare = {
  user: { id: 'max@example.org', name: 'max@example.org', email: 'max@example.org' },
  right: null,
  pending: true,
};

const shareReads = () => invokeMock.mock.calls.filter(([c]) => c === 'task_list_shares').length;

/** A read the test answers when it says so. */
function heldRead(rows: TaskListShare[]) {
  let release = () => {};
  const read = new Promise<TaskListShare[]>((resolve) => {
    release = () => resolve(rows);
  });
  return { read, release };
}

const loadingLine = () => screen.queryByText('Mitglieder werden geladen …');

beforeEach(() => {
  state.reads = [];
  state.failure = null;
});
afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockClear();
});

async function open(capabilities?: { member_add_by: 'email' }) {
  const { TaskMembersDialog } = await import('./TaskMembersDialog');
  render(
    <TaskMembersDialog
      isOpen
      onClose={() => {}}
      listId="L1"
      listName="Haushalt"
      capabilities={capabilities as never}
    />,
  );
}

describe('TaskMembersDialog → a write that may have landed', () => {
  it('shows the role the provider holds after a lost answer', async () => {
    // The provider holds another role than the one chosen: the read is what
    // the select shows.
    const second = heldRead([erika('read')]);
    state.reads = [[erika('write')], second.read];
    state.failure = { code: 'network', message: 'timeout' };
    await open();
    const select = (await screen.findByLabelText('Recht für Erika')) as HTMLSelectElement;
    select.focus();
    fireEvent.change(select, { target: { value: 'admin' } });
    await waitFor(() => expect(shareReads()).toBe(2));
    // Read quietly (review round 22 of #128): no loading line while it reads,
    // the same select, and the focus with it.
    expect(loadingLine()).toBeNull();
    expect(screen.getByLabelText('Recht für Erika')).toBe(select);
    expect(document.activeElement).toBe(select);
    await act(async () => second.release());
    await waitFor(() => expect(screen.getByRole('alert')).toBeTruthy());
    const after = screen.getByLabelText('Recht für Erika') as HTMLSelectElement;
    expect(after.value).toBe('read');
    expect(after).toBe(select);
    expect(document.activeElement).toBe(select);
  });

  it('reads the members again quietly after a removal whose answer was lost', async () => {
    // Review round 22 of #128: with the loading line, the row went, and the
    // focus on its button with it.
    const second = heldRead([erika('write')]);
    state.reads = [[erika('write')], second.read];
    state.failure = { code: 'network', message: 'timeout' };
    await open();
    await screen.findByLabelText('Recht für Erika');
    fireEvent.click(screen.getByRole('button', { name: 'Entfernen' }));
    await waitFor(() => expect(shareReads()).toBe(2));
    expect(loadingLine()).toBeNull();
    await act(async () => second.release());
    await waitFor(() => expect(screen.getByRole('alert')).toBeTruthy());
    expect(screen.getByRole('button', { name: 'Entfernen' })).toBeTruthy();
  });

  it('puts the old role back where the change was turned away', async () => {
    state.reads = [[erika('write')]];
    state.failure = { code: 'forbidden', message: 'access denied' };
    await open();
    const select = (await screen.findByLabelText('Recht für Erika')) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'admin' } });
    await waitFor(() => expect(screen.getByRole('alert')).toBeTruthy());
    expect((screen.getByLabelText('Recht für Erika') as HTMLSelectElement).value).toBe('write');
    expect(shareReads()).toBe(1);
  });

  it('lists a member whose invitation went in although its answer was lost', async () => {
    state.reads = [[erika('write')], [erika('write'), max]];
    state.failure = { code: 'network', message: 'timeout' };
    await open({ member_add_by: 'email' });
    await screen.findByLabelText('Recht für Erika');
    fireEvent.change(screen.getByPlaceholderText('E-Mail-Adresse …'), {
      target: { value: 'max@example.org' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Einladen' }));
    await waitFor(() => expect(shareReads()).toBe(2));
    expect(await screen.findByText('max@example.org')).toBeTruthy();
    // The write's failure is still said: the read after it did not clear it.
    expect(screen.getByRole('alert')).toBeTruthy();
  });
});
