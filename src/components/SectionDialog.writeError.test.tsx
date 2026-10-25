import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

/**
 * The section dialog's colour is a write of its own after the section's
 * (decision 277): turned away once the section landed, it says the change is
 * partly saved, with Close only — creating the section again would make two.
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

const BUSY = { code: 'forbidden', message: 'provider-busy: http-429' };
const EINKAUF = { id: 's1', list_id: 'L1', name: 'Einkauf', position: 0, color_label: null };

afterEach(() => {
  document.body.innerHTML = '';
  invokeMock.mockReset();
  loadSections.mockClear();
  announce.mockClear();
});

describe('SectionDialog → a colour turned away after the section landed', () => {
  it('closes into "partly saved", never "nothing was changed"', async () => {
    invokeMock.mockImplementation((command: string) =>
      command === 'create_section'
        ? Promise.resolve(EINKAUF)
        : command === 'set_section_color'
          ? Promise.reject(BUSY)
          : Promise.resolve([]),
    );
    const onClose = vi.fn();
    const { SectionDialog } = await import('./SectionDialog');
    render(<SectionDialog isOpen onClose={onClose} listId="L1" section={null} />);
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Einkauf' } });
    fireEvent.change(screen.getByLabelText('Abschnittsfarbe'), { target: { value: 'red' } });
    fireEvent.click(screen.getByRole('button', { name: 'Anlegen' }));
    expect(
      await screen.findByText(
        'Die Änderung ist nur zum Teil gespeichert, weil der Anbieter gerade ausgelastet ist. ' +
          'Prüfe sie und versuche den Rest später noch einmal.',
      ),
    ).toBeTruthy();
    // The section shows as it stands: without the colour.
    expect(loadSections).toHaveBeenCalledWith('L1', { upsert: EINKAUF });
    expect(screen.queryByRole('button', { name: 'Anlegen' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Schließen' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
