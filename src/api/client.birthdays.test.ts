import { beforeEach, describe, expect, it, vi } from 'vitest';

// The Tauri entry point, mocked: this test drives the real `listCalendars`
// wrapper with rows shaped exactly as `host_core::wire::CalendarRow` answers.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { DEFAULT_RECURRENCE_CAPABILITIES } from '@aperio/shared';
import { invoke } from '@tauri-apps/api/core';

import { listCalendars } from './client';

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

const row = (over: Record<string, unknown>) => ({
  id: 'x',
  name: 'x',
  color: null,
  color_label: null,
  read_only: false,
  default_sound: null,
  supports_scheduling: false,
  supports_event_color: false,
  account_id: 'local',
  recurrence_capabilities: DEFAULT_RECURRENCE_CAPABILITIES,
  ...over,
});

beforeEach(() => {
  invokeMock.mockReset();
});

// The name a birthday calendar is shown with is built on THIS side now: the
// host names the calendar after its address book and says, on the row, that it
// is a birthday layer. The shared function is tested in
// `src/state/birthdays.test.ts`; this pins that the loading boundary every
// desktop consumer reads through actually calls it — a wrapper that forgot to
// would show "Familie" instead of "Geburtstage – Familie", silently.
describe('listCalendars — a birthday calendar is named in the UI language', () => {
  it('builds the name from the layer, and leaves other calendars alone', async () => {
    const work = row({ id: 'caldav-1', name: 'Arbeit' });
    invokeMock.mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === 'list_calendars'
          ? [
              work,
              row({
                id: 'aperio-birthdays:list-1',
                name: 'Familie',
                read_only: true,
                birthdays: { contact_list_id: 'list-1', list_name: 'Familie' },
              }),
            ]
          : [],
      ),
    );

    const calendars = await listCalendars();

    // The test setup runs the app in German.
    expect(calendars.map((c) => c.name)).toEqual(['Arbeit', 'Geburtstage – Familie']);
    expect(calendars[0]).toBe(work);
  });
});
