import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

// Mock the command + Tauri-event boundaries so the section renders without a
// backend. `listen` resolves to a no-op unlisten (the component calls it on
// mount and unmount).
vi.mock('../api/client', () => ({
  listSyncLogEntries: vi.fn(),
  clearSyncLog: vi.fn(() => Promise.resolve()),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

import { listSyncLogEntries, type SyncLogEntry } from '../api/client';
import { AnnouncerProvider } from '../a11y/Announcer';
import { SyncProtocolSection } from './SyncProtocolSection';
// Side-effect import initialises the shared i18next instance so the aria
// labels resolve the dialogs.settings.sync.protocol* keys.
import '../i18n';

const listMock = listSyncLogEntries as unknown as ReturnType<typeof vi.fn>;

const entry = (over: Partial<SyncLogEntry>): SyncLogEntry => ({
  id: 1,
  recorded_at: '2026-07-05T09:00:00Z',
  trigger: 'periodic',
  success: true,
  pushed_logs: 1,
  fetched_logs: 0,
  applied: 0,
  conflicts: 0,
  duration_ms: 42,
  error: null,
  ...over,
});

function renderSection() {
  return render(
    <AnnouncerProvider>
      <SyncProtocolSection headingId="proto-h" />
    </AnnouncerProvider>,
  );
}

afterEach(() => {
  document.body.innerHTML = '';
  listMock.mockReset();
});

describe('SyncProtocolSection', () => {
  it('renders the log as a single-tab-stop listbox of option rows', async () => {
    listMock.mockResolvedValue([
      entry({ id: 1, trigger: 'manual' }),
      entry({ id: 2, trigger: 'kick', success: false, error: 'boom' }),
    ]);
    renderSection();

    const listbox = await screen.findByRole('listbox');
    // ONE tab stop for the whole list (the fix: rows were unreachable <li>s).
    expect(listbox).toHaveAttribute('tabindex', '0');
    const options = await screen.findAllByRole('option');
    expect(options).toHaveLength(2);

    // Each option folds the whole round into its accessible name, so a
    // focus-mode screen reader reads it on arrow. The failure row carries
    // its "Fehlgeschlagen" status in the summary.
    expect(options[0]).toHaveAccessibleName(/Manuell/);
    expect(options[1]).toHaveAccessibleName(/Fehlgeschlagen: boom/);
  });

  it('moves the active option with the arrow keys, gated on focus', async () => {
    listMock.mockResolvedValue([
      entry({ id: 1, trigger: 'manual' }),
      entry({ id: 2, trigger: 'periodic' }),
    ]);
    renderSection();

    const listbox = await screen.findByRole('listbox');
    // Before focus, nothing is active — else NVDA reads row 1 the moment the
    // list enters the a11y tree while focus is still on the Settings tab.
    expect(listbox).not.toHaveAttribute('aria-activedescendant');

    fireEvent.focus(listbox);
    const options = screen.getAllByRole('option');
    expect(listbox).toHaveAttribute('aria-activedescendant', options[0].id);

    fireEvent.keyDown(listbox, { key: 'ArrowDown' });
    expect(listbox).toHaveAttribute('aria-activedescendant', options[1].id);
    expect(options[1]).toHaveAttribute('aria-selected', 'true');

    // End/Home jump to the last/first row.
    fireEvent.keyDown(listbox, { key: 'Home' });
    expect(listbox).toHaveAttribute('aria-activedescendant', options[0].id);
  });

  it('shows the empty hint (no listbox) when there are no rounds', async () => {
    listMock.mockResolvedValue([]);
    renderSection();

    await waitFor(() => expect(listMock).toHaveBeenCalled());
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(screen.getByText('Noch keine Sync-Runden.')).toBeTruthy();
  });
});
