import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

import type { FileImport, ImportedEvent } from '../api/types';

/**
 * Bringing a file in (DESIGN-file-import.md): the open dialog filters by the
 * formats the plugins read; one appointment opens the editor on it, several,
 * none or a failure open the import dialog; a dropped file is read from its
 * bytes.
 */

const answers = vi.hoisted(() => ({
  read: null as unknown,
  readFails: null as unknown,
}));
const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) => {
    if (command === 'file_import_formats') {
      return Promise.resolve([
        {
          plugin_id: 'com.aperio.cal-adapter-ical',
          format_id: 'ics',
          name: 'iCalendar-Datei (.ics)',
          extensions: ['ics', 'ical'],
          mime_types: ['text/calendar'],
        },
      ]);
    }
    if (command === 'read_import_file' || command === 'read_import_content') {
      return answers.readFails ? Promise.reject(answers.readFails) : Promise.resolve(answers.read);
    }
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
const openDialog = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: openDialog }));
const openEventDialog = vi.fn();
const openFileImport = vi.fn();
vi.mock('./dialogStateContext', () => ({
  useDialogState: () => ({ openEventDialog, openFileImport }),
}));

afterEach(() => {
  invokeMock.mockClear();
  openDialog.mockReset();
  openEventDialog.mockClear();
  openFileImport.mockClear();
  answers.read = null;
  answers.readFails = null;
});

const event = (title: string, extra: Partial<ImportedEvent> = {}): ImportedEvent => ({
  title,
  start: '2026-11-18T13:00:00Z',
  end: '2026-11-18T14:00:00Z',
  all_day: false,
  reminders: [],
  attendees: [],
  ...extra,
});

async function hook() {
  const { useFileImport } = await import('./useFileImport');
  return renderHook(() => useFileImport()).result.current;
}

describe('useFileImport', () => {
  it('filters the open dialog by the plugins’ formats and opens the editor on one appointment', async () => {
    openDialog.mockResolvedValue('C:\\Daten\\Einladung.ics');
    answers.read = {
      events: [event('Projektbesprechung', { organizer: 'erika@example.org', attendees: ['a@b.de'] })],
      skipped: [{ kind: 'task', title: 'Geschenke besorgen' }],
    } satisfies FileImport;
    const { importFromDisk } = await hook();
    await importFromDisk();
    const options = openDialog.mock.calls[0][0];
    expect(options.filters[0]).toEqual({ name: 'Kalenderdateien', extensions: ['ics', 'ical'] });
    expect(invokeMock).toHaveBeenCalledWith('read_import_file', {
      path: 'C:\\Daten\\Einladung.ics',
    });
    expect(openEventDialog).toHaveBeenCalledTimes(1);
    const [, opened] = openEventDialog.mock.calls[0];
    expect(opened.importFrom.title).toBe('Projektbesprechung');
    // The invitation is set aside until each provider keeps it quietly (I6).
    expect(opened.importFrom.attendees).toEqual([]);
    expect(opened.importNote).toBe(
      'Nicht übernommen: die Aufgabe „Geschenke besorgen“. Teilnehmer und Organisator dieser ' +
        'Einladung übernimmt Aperio noch nicht; der Termin wird dein eigener, ohne Teilnehmer.',
    );
    expect(openFileImport).not.toHaveBeenCalled();
  });

  it('lists several appointments, and a failure, in the import dialog', async () => {
    openDialog.mockResolvedValue('/home/toni/export.ics');
    answers.read = { events: [event('A'), event('B')], skipped: [] };
    const { importFromDisk } = await hook();
    await importFromDisk();
    expect(openFileImport).toHaveBeenCalledWith('export.ics', { read: answers.read });

    answers.readFails = { code: 'invalid_input', message: 'the file holds no calendar' };
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await importFromDisk();
    expect(openFileImport).toHaveBeenLastCalledWith('export.ics', { failed: answers.readFails });
    warn.mockRestore();
  });

  it('does nothing when the open dialog is cancelled', async () => {
    openDialog.mockResolvedValue(null);
    const { importFromDisk } = await hook();
    await importFromDisk();
    expect(invokeMock).not.toHaveBeenCalledWith('read_import_file', expect.anything());
    expect(openEventDialog).not.toHaveBeenCalled();
    expect(openFileImport).not.toHaveBeenCalled();
  });

  it('reads a dropped file from its bytes', async () => {
    answers.read = { events: [event('A'), event('B')], skipped: [] };
    const { importDropped } = await hook();
    const file = new File(['BEGIN:VCALENDAR'], 'drop.ics', { type: 'text/calendar' });
    await importDropped(file);
    expect(invokeMock).toHaveBeenCalledWith('read_import_content', {
      fileName: 'drop.ics',
      content: btoa('BEGIN:VCALENDAR'),
    });
    expect(openFileImport).toHaveBeenCalledWith('drop.ics', { read: answers.read });
  });
});

describe('toBase64', () => {
  it('encodes more bytes than one call may take', async () => {
    const { toBase64 } = await import('./useFileImport');
    const bytes = new Uint8Array(100_000).map((_, i) => i % 256);
    const decoded = Uint8Array.from(atob(toBase64(bytes)), (c) => c.charCodeAt(0));
    expect(decoded).toEqual(bytes);
  });
});
