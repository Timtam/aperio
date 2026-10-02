import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

import type { Calendar, CalendarEvent } from '../api/types';

/**
 * Toni's report: in the quick-add, the offer named the right calendar, and
 * the editor it opened ended on the TOP calendar (decisions 160 and 161).
 *
 * The whole path runs here — quick-add, accept, the editor it hands over to —
 * with only the wire faked, so what the editor's picker ends on is what the
 * user would hear on tabbing to it.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((command: string) => {
    if (command === 'get_user_pref') return Promise.resolve(null);
    return Promise.resolve([]);
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const LOCAL = { id: 'cal-local', name: 'Kalender', read_only: false } as unknown as Calendar;
const WORK = { id: 'cal-work', name: 'Arbeit', read_only: false } as unknown as Calendar;
const HOME = { id: 'cal-home', name: 'Privat', read_only: false } as unknown as Calendar;
const FEED = { id: 'cal-feed', name: 'Abo', read_only: true } as unknown as Calendar;

/** Not 60 minutes and not empty, so a refused fill can never look like the
 *  editor's own baseline (which would hide a refusal behind equality). */
function ev(id: string, calendar: string, start: string): CalendarEvent {
  return {
    id,
    calendar_id: calendar,
    title: 'Thomas Meeting',
    description: 'Agenda',
    location: null,
    start,
    end: new Date(Date.parse(start) + 45 * 60_000).toISOString(),
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    attendees: [],
  } as unknown as CalendarEvent;
}

const STATE: { calendars: Calendar[]; matches: CalendarEvent[] } = {
  calendars: [LOCAL, WORK, HOME],
  matches: [],
};
const STORE = {
  get calendars() {
    return STATE.calendars;
  },
  colorLabels: [],
  selectedCalendarIds: new Set(['cal-local', 'cal-work', 'cal-home', 'cal-feed']),
};
const VIEW_STATE = {
  anchor: new Date('2026-06-20T08:00:00'),
  showHiddenCalendarTargets: false,
};
const announce = vi.hoisted(() => vi.fn());

vi.mock('../state/calendarStoreContext', () => ({
  useCalendarStore: () => STORE,
}));
vi.mock('../state/viewStateContext', () => ({ useViewState: () => VIEW_STATE }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => announce }));
vi.mock('../state/useTitleSuggestions', async () => {
  const actual = await vi.importActual<
    typeof import('../state/useTitleSuggestions')
  >('../state/useTitleSuggestions');
  return { ...actual, useTitleSuggestions: () => STATE.matches };
});

afterEach(() => {
  document.body.innerHTML = '';
  announce.mockClear();
  STATE.calendars = [LOCAL, WORK, HOME];
  STATE.matches = [];
});

async function mount(open: 'quickAdd' | 'editor') {
  const { DialogStateProvider } = await import('../state/DialogState');
  const { useDialogState } = await import('../state/dialogStateContext');
  const { DialogHost } = await import('./DialogHost');

  function Opener() {
    const { openQuickAdd, openEventDialog } = useDialogState();
    return (
      <button
        type="button"
        onClick={() => (open === 'quickAdd' ? openQuickAdd({}) : openEventDialog(null))}
      >
        open
      </button>
    );
  }

  const tree = () => (
    <DialogStateProvider>
      <Opener />
      <DialogHost />
    </DialogStateProvider>
  );
  const view = render(tree());
  fireEvent.click(screen.getByRole('button', { name: 'open' }));
  return () => view.rerender(tree());
}

/** Type, take the first offer, and return what it said and where the
 *  editor's calendar picker ends once every late read has landed. */
async function acceptFirstOffer(beforeTyping?: () => void) {
  const title = await screen.findByRole('combobox', { name: /titel/i });
  beforeTyping?.();
  fireEvent.change(title, { target: { value: 'thomas' } });
  const option = await screen.findByRole('option', { name: /thomas/i });
  const offered = option.textContent;
  fireEvent.keyDown(title, { key: 'ArrowDown' });
  fireEvent.keyDown(title, { key: 'Enter' });
  const picker = (await screen.findByRole('combobox', {
    name: /kalender/i,
  })) as HTMLSelectElement;
  await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
  return { offered, picker };
}

const spoken = () => announce.mock.calls.map((c) => String(c[0]));

describe('quick-add → accept an offer → the editor', () => {
  it("opens the editor on the calendar the offer named", async () => {
    STATE.matches = [ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z')];
    await mount('quickAdd');
    const { offered, picker } = await acceptFirstOffer();
    expect(offered).toContain('Arbeit');
    expect(picker.value).toBe('cal-work');
  }, 15_000);

  it('takes the copy the offer named when the same id sits in two calendars', async () => {
    // Google keeps an event's id across calendars; a copy keeps its UID. The
    // search returns the older copy on the top calendar first.
    STATE.matches = [
      ev('ev-1', 'cal-local', '2026-05-15T09:00:00.000Z'),
      ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z'),
    ];
    await mount('quickAdd');
    const { offered, picker } = await acceptFirstOffer();
    expect(offered).toContain('Arbeit');
    expect(picker.value).toBe('cal-work');
  }, 15_000);

  it('does not count arrowing through the picker and back as a choice', async () => {
    STATE.matches = [ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z')];
    await mount('quickAdd');
    const { picker } = await acceptFirstOffer(() => {
      // A closed select fires a change per arrow key.
      const select = screen.getByRole('combobox', { name: /kalender/i });
      fireEvent.change(select, { target: { value: 'cal-work' } });
      fireEvent.change(select, { target: { value: 'cal-local' } });
    });
    expect(picker.value).toBe('cal-work');
  }, 15_000);

  it('keeps a calendar the user left the picker on', async () => {
    STATE.matches = [ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z')];
    await mount('quickAdd');
    const { picker } = await acceptFirstOffer(() => {
      const select = screen.getByRole('combobox', { name: /kalender/i });
      fireEvent.change(select, { target: { value: 'cal-home' } });
    });
    expect(picker.value).toBe('cal-home');
  }, 15_000);

  it('offers a writable copy before a newer one in a read-only calendar', async () => {
    STATE.calendars = [LOCAL, WORK, HOME, FEED];
    STATE.matches = [
      ev('feed-1', 'cal-feed', '2026-06-15T09:00:00.000Z'),
      ev('own-1', 'cal-work', '2026-05-15T09:00:00.000Z'),
    ];
    await mount('quickAdd');
    const { offered, picker } = await acceptFirstOffer();
    expect(offered).toContain('Arbeit');
    expect(picker.value).toBe('cal-work');
  }, 15_000);

  it('says which calendar it takes when the offer can only come from a read-only one', async () => {
    STATE.calendars = [LOCAL, WORK, HOME, FEED];
    STATE.matches = [ev('feed-1', 'cal-feed', '2026-06-15T09:00:00.000Z')];
    await mount('quickAdd');
    const { offered, picker } = await acceptFirstOffer();
    expect(offered).toContain('Abo, nur lesbar');
    expect(picker.value).toBe('cal-local');
    // The editor says the fill and the refusal as one sentence: the
    // quick-add's own "filled in" comes in the same frame and is replaced.
    expect(spoken().at(-1)).toBe(
      'Aus „Thomas Meeting" übernommen. Der Tag bleibt, wie er war. ' +
        '„Abo“ nimmt keine neuen Termine an. Der Termin kommt in „Kalender“.',
    );
  }, 15_000);

  it('says the refusal once, however often the catalog refreshes', async () => {
    // A bare hour with nothing else: filled from it, the editor's form equals
    // its own baseline, so a catalog refresh resets it as untouched and the
    // prefill lands again.
    STATE.calendars = [LOCAL, WORK, HOME, FEED];
    const bare = {
      ...ev('feed-1', 'cal-feed', '2026-06-15T09:00:00.000Z'),
      description: null,
      end: '2026-06-15T10:00:00.000Z',
    } as CalendarEvent;
    STATE.matches = [bare];
    const rerender = await mount('quickAdd');
    await acceptFirstOffer();
    const refusals = () => spoken().filter((s) => s.includes('nimmt keine')).length;
    expect(refusals()).toBe(1);
    STATE.calendars = [...STATE.calendars];
    rerender();
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    expect(refusals()).toBe(1);
  }, 15_000);
});

describe("the editor's own title field", () => {
  it('takes the copy the offer named, and says the refusal with the fill', async () => {
    STATE.calendars = [LOCAL, WORK, HOME, FEED];
    STATE.matches = [ev('feed-1', 'cal-feed', '2026-06-15T09:00:00.000Z')];
    await mount('editor');
    const { picker } = await acceptFirstOffer();
    expect(picker.value).toBe('cal-local');
    // ONE announcement: a second in the same frame would replace the first.
    const filled = spoken().filter((s) => s.includes('übernommen'));
    expect(filled).toEqual([
      'Aus „Thomas Meeting" übernommen. Der Tag bleibt, wie er war. ' +
        '„Abo“ nimmt keine neuen Termine an. Der Termin kommt in „Kalender“.',
    ]);
  }, 15_000);

  it('takes the named copy when the same id sits in two calendars', async () => {
    STATE.matches = [
      ev('ev-1', 'cal-local', '2026-05-15T09:00:00.000Z'),
      ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z'),
    ];
    await mount('editor');
    const { picker } = await acceptFirstOffer();
    expect(picker.value).toBe('cal-work');
  }, 15_000);

  it('does not offer the same entries again after one was taken', async () => {
    STATE.matches = [ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z')];
    await mount('editor');
    await acceptFirstOffer();
    // The fill changed the title, so the offers come again for it: they must
    // not pop up by themselves and announce a count over the fill.
    STATE.matches = [ev('ev-1', 'cal-work', '2026-06-15T09:00:00.000Z')];
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(spoken().some((s) => s.includes('frühere'))).toBe(true);
    const afterFill = spoken().slice(
      spoken().findIndex((s) => s.includes('übernommen')),
    );
    expect(afterFill.some((s) => s.includes('frühere'))).toBe(false);
  }, 15_000);
});
