import { useEffect, useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import type { Signature } from '@aperio/shared';

/**
 * The Signatures panel is the shared master/detail selector: ONE listbox
 * (one tab stop) plus a detail pane for the selected signature only. Before,
 * every signature rendered its own name field, text area and delete button —
 * three tab stops each, none of them naming the signature they edited.
 */

// A stateful stand-in for useSignatures: `save` updates a store that the mocked
// hook subscribes to, so a delete actually re-renders the panel the way the
// real hook's optimistic update does.
const store = vi.hoisted(() => {
  const listeners = new Set<() => void>();
  const s = {
    signatures: [] as Signature[],
    set(next: Signature[]) {
      s.signatures = next;
      listeners.forEach((l) => l());
    },
    subscribe(l: () => void) {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    save: vi.fn((next: Signature[]) => {
      s.set(next);
      return Promise.resolve();
    }),
  };
  return s;
});
vi.mock('../state/useSignatures', () => ({
  useSignatures: () => {
    const [, bump] = useState(0);
    useEffect(() => store.subscribe(() => bump((n) => n + 1)), []);
    return { signatures: store.signatures, loading: false, save: store.save };
  },
}));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

import { SignaturesPanel } from './SignaturesPanel';

afterEach(() => {
  document.body.innerHTML = '';
  store.save.mockClear();
});

const THREE: Signature[] = [
  { id: 's1', name: 'Raum 12', body: 'Zugang: 4711\nBitte klingeln' },
  { id: 's2', name: 'Abteilung', body: '' },
  { id: 's3', name: 'Hinweis', body: 'Nur Klartext' },
];

describe('SignaturesPanel', () => {
  it('lists every signature in one listbox and edits only the selected one', () => {
    store.set(THREE);
    render(<SignaturesPanel />);

    const listbox = screen.getByRole('listbox', { name: 'Signaturen' });
    const options = within(listbox).getAllByRole('option');
    expect(options).toHaveLength(3);
    // Name plus a summary of the text — the first non-empty line, or a
    // stand-in for an empty body — so the reader knows which is which.
    expect(options[0]).toHaveAccessibleName('Raum 12, Zugang: 4711');
    expect(options[1]).toHaveAccessibleName('Abteilung, noch kein Text');

    // Exactly one detail pane, for the selected signature, and exactly one
    // set of edit controls in the whole panel (the add form's are separate).
    expect(screen.getByRole('heading', { name: 'Signatur Raum 12' })).toBeTruthy();
    const textareas = screen
      .getAllByRole('textbox')
      .filter((el) => el.tagName === 'TEXTAREA');
    expect(textareas).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: /Signatur löschen/ })).toHaveLength(1);

    // Arrowing moves the selection and swaps the detail.
    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(screen.getByRole('heading', { name: 'Signatur Abteilung' })).toBeTruthy();
    expect(listbox).toHaveAttribute('aria-activedescendant', options[1].id);
  });

  it('writes an edited name on blur for the selected signature only', () => {
    store.set([
      { id: 's1', name: 'Raum 12', body: 'x' },
      { id: 's2', name: 'Abteilung', body: 'y' },
    ]);
    render(<SignaturesPanel />);
    const detail = screen.getByRole('region', { name: 'Signatur Raum 12' });
    const nameField = within(detail).getByRole('textbox', { name: 'Name' });
    fireEvent.change(nameField, { target: { value: 'Raum 13' } });
    fireEvent.blur(nameField);
    expect(store.save).toHaveBeenCalledWith([
      { id: 's1', name: 'Raum 13', body: 'x' },
      { id: 's2', name: 'Abteilung', body: 'y' },
    ]);
  });

  it('after a delete, focus lands on the list and the selection moves to the neighbour', async () => {
    store.set(THREE);
    render(<SignaturesPanel />);
    const listbox = screen.getByRole('listbox', { name: 'Signaturen' });
    // Select the middle one, delete it.
    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    const del = screen.getByRole('button', { name: 'Signatur löschen: Abteilung' });
    del.focus();
    await act(async () => {
      fireEvent.click(del);
    });
    // The delete button unmounted with its pane; focus must not fall to
    // <body> (that drops a screen reader out of application mode).
    await waitFor(() => expect(document.activeElement).toBe(listbox));
    // The selection moved to the deleted one's NEXT neighbour, not the top.
    expect(screen.getByRole('heading', { name: 'Signatur Hinweis' })).toBeTruthy();
    expect(within(listbox).getAllByRole('option')).toHaveLength(2);
  });

  it('deleting the last signature parks focus on the panel, not on <body>', async () => {
    store.set([{ id: 's1', name: 'Einzige', body: 'x' }]);
    const { container } = render(<SignaturesPanel />);
    const del = screen.getByRole('button', { name: 'Signatur löschen: Einzige' });
    del.focus();
    await act(async () => {
      fireEvent.click(del);
    });
    // The list is gone too (empty state) — the panel container is the home.
    expect(screen.queryByRole('listbox')).toBeNull();
    await waitFor(() =>
      expect(document.activeElement).toBe(container.querySelector('.settings-panel')),
    );
  });
});
