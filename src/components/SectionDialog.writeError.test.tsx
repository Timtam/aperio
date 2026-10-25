import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

/**
 * The section dialog's colour is a write of its own after the section's
 * (decision 277), a local one: failing once the section landed, it never says
 * "nothing was changed", and the dialog shows it with Close only — creating
 * the section again would make two.
 */

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

const loadSections = vi.fn(() => Promise.resolve([]));
const STORE = {
  colorLabels: [{ id: 'red', name: 'Rot', color: '#ff0000', position: 0 }],
  sectionsByList: {},
  loadSections,
};
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => STORE }));
const announce = vi.fn();
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));

const LOCKED = { code: 'internal', message: 'database is locked' };
const EINKAUF = { id: 's1', list_id: 'L1', name: 'Einkauf', position: 0, color_label: null };

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockReset();
  loadSections.mockClear();
  announce.mockClear();
});

describe('SectionDialog → a colour that failed after the section landed', () => {
  it('closes into a notice that never says "nothing was changed"', async () => {
    invokeMock.mockImplementation((command: string) =>
      command === 'create_section'
        ? Promise.resolve(EINKAUF)
        : command === 'set_section_color'
          ? Promise.reject(LOCKED)
          : Promise.resolve([]),
    );
    const onClose = vi.fn();
    const { SectionDialog } = await import('./SectionDialog');
    render(<SectionDialog isOpen onClose={onClose} listId="L1" section={null} />);
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Einkauf' } });
    fireEvent.change(screen.getByLabelText('Abschnittsfarbe'), { target: { value: 'red' } });
    fireEvent.click(screen.getByRole('button', { name: 'Anlegen' }));
    expect(
      await screen.findByText('network: section-color: internal error: database is locked'),
    ).toBeTruthy();
    // The section shows as it stands: without the colour.
    expect(loadSections).toHaveBeenCalledWith('L1', { upsert: EINKAUF });
    expect(screen.queryByRole('button', { name: 'Anlegen' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Schließen' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
