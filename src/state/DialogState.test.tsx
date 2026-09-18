import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

import type { CalendarEvent } from '../api/types';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import { DialogStateProvider } from './DialogState';
import { useDialogState } from './dialogStateContext';

// A synthetic expanded occurrence carries a `series_id` — that's what
// isExpandedOccurrence keys on, and what triggers the up-front scope prompt.
const occurrence = {
  id: 'evt-1@2026-07-20T08:30:00Z',
  series_id: 'evt-1',
  calendar_id: 'cal-1',
  title: 'Tabletten nehmen',
} as unknown as CalendarEvent;

/** The series the occurrence belongs to, as `get_event_by_id` returns it. */
const series = {
  id: 'evt-1',
  calendar_id: 'cal-1',
  title: 'Tabletten nehmen',
  start: '2026-07-01T08:30:00Z',
  recurrence: { rrule: 'FREQ=DAILY', exceptions: [] },
} as unknown as CalendarEvent;

// A plain, non-recurring row (no series_id) opens the editor directly.
const single = {
  id: 'evt-2',
  calendar_id: 'cal-1',
  title: 'Zahnarzt',
} as unknown as CalendarEvent;

function Probe() {
  const d = useDialogState();
  const m = d.mode;
  return (
    <div>
      <span data-testid="kind">{m.kind}</span>
      <span data-testid="scope">
        {m.kind === 'event' ? (m.initialScope ?? 'none') : ''}
      </span>
      <span data-testid="event">{m.kind === 'event' ? (m.event?.id ?? '') : ''}</span>
      <span data-testid="failed">
        {m.kind === 'eventEditScope' && m.seriesLoadFailed ? String(m.seriesLoadFailed) : ''}
      </span>
      <button type="button" onClick={() => d.openEventDialog(occurrence)}>
        open-occ
      </button>
      <button type="button" onClick={() => d.openEventDialog(single)}>
        open-single
      </button>
      <button type="button" onClick={() => d.chooseEventEditScope('occurrence')}>
        choose-occ
      </button>
      <button type="button" onClick={() => d.chooseEventEditScope('series')}>
        choose-series
      </button>
      <button type="button" onClick={() => d.close()}>
        cancel
      </button>
    </div>
  );
}

/** Click, and let the series load that a click may start settle. */
async function click(label: string) {
  await act(async () => {
    screen.getByText(label).click();
  });
}

function renderProbe() {
  render(
    <DialogStateProvider>
      <Probe />
    </DialogStateProvider>,
  );
}

/** A load the test answers itself, when it chooses. */
function pendingLoad() {
  let answer: (value: CalendarEvent | null) => void = () => {};
  const promise = new Promise<CalendarEvent | null>((resolve) => {
    answer = resolve;
  });
  return { promise, answer: (value: CalendarEvent | null) => answer(value) };
}

const seriesLoads = () =>
  invokeMock.mock.calls.filter((call) => call[0] === 'get_event_by_id').length;

afterEach(() => {
  invokeMock.mockReset();
});

describe('DialogState recurring-edit scope prompt', () => {
  it('opening a recurring occurrence shows the scope prompt first', async () => {
    renderProbe();
    expect(screen.getByTestId('kind').textContent).toBe('none');
    await click('open-occ');
    expect(screen.getByTestId('kind').textContent).toBe('eventEditScope');
  });

  it('choosing "series" opens the editor on the series itself', async () => {
    // The occurrence's start is not the series' start; an editor filled from
    // it moved the whole series to that occurrence when saved.
    invokeMock.mockResolvedValue(series);
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    expect(invokeMock).toHaveBeenCalledWith('get_event_by_id', {
      id: 'evt-1',
      calendarId: 'cal-1',
    });
    expect(screen.getByTestId('kind').textContent).toBe('event');
    expect(screen.getByTestId('event').textContent).toBe('evt-1');
    // The choice rides along, so the editor can name it: the series itself is
    // no occurrence, and nothing else would tell this edit from any other.
    expect(screen.getByTestId('scope').textContent).toBe('series');
  });

  it('keeps the prompt and says so when the series cannot be loaded', async () => {
    invokeMock.mockResolvedValue(null);
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    expect(screen.getByTestId('kind').textContent).toBe('eventEditScope');
    expect(screen.getByTestId('failed').textContent).toBe('1');
  });

  it('keeps the prompt when loading the series throws', async () => {
    invokeMock.mockRejectedValue(new Error('offline'));
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    expect(screen.getByTestId('kind').textContent).toBe('eventEditScope');
    expect(screen.getByTestId('failed').textContent).toBe('1');
  });

  it('counts a retry that fails too, so it is announced again', async () => {
    invokeMock.mockResolvedValue(null);
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    await click('choose-series');
    expect(screen.getByTestId('failed').textContent).toBe('2');
  });

  it('starts no second load while the series is loading', async () => {
    // Two loads could land in either order; a failure landing first replaced
    // the prompt, and the success that followed was dropped.
    const load = pendingLoad();
    invokeMock.mockReturnValue(load.promise);
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    await click('choose-series');
    expect(seriesLoads()).toBe(1);
    await act(async () => {
      load.answer(series);
    });
    expect(screen.getByTestId('kind').textContent).toBe('event');
    expect(screen.getByTestId('event').textContent).toBe('evt-1');
  });

  it('a load for a dismissed prompt does not unblock the prompt that replaced it', async () => {
    const first = pendingLoad();
    const second = pendingLoad();
    invokeMock.mockReturnValueOnce(first.promise).mockReturnValue(second.promise);
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    await click('cancel');
    await click('open-occ');
    await click('choose-series');
    await act(async () => {
      first.answer(series);
    });
    // The first prompt's load landed; the second prompt's is still running.
    await click('choose-series');
    expect(seriesLoads()).toBe(2);
    await act(async () => {
      second.answer(series);
    });
    expect(screen.getByTestId('event').textContent).toBe('evt-1');
  });

  it('opens nothing when the prompt was cancelled while the series loaded', async () => {
    const load = pendingLoad();
    invokeMock.mockReturnValue(load.promise);
    renderProbe();
    await click('open-occ');
    await click('choose-series');
    await click('cancel');
    await act(async () => {
      load.answer(series);
    });
    expect(screen.getByTestId('kind').textContent).toBe('none');
  });

  it('choosing "this occurrence" hands off scoped to the occurrence', async () => {
    renderProbe();
    await click('open-occ');
    await click('choose-occ');
    expect(screen.getByTestId('kind').textContent).toBe('event');
    expect(screen.getByTestId('scope').textContent).toBe('occurrence');
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('cancelling the prompt opens no editor', async () => {
    renderProbe();
    await click('open-occ');
    await click('cancel');
    expect(screen.getByTestId('kind').textContent).toBe('none');
  });

  it('a non-recurring event opens the editor directly, no prompt', async () => {
    renderProbe();
    await click('open-single');
    expect(screen.getByTestId('kind').textContent).toBe('event');
    // No prompt ⇒ no forced scope; the editor falls back to its own default.
    expect(screen.getByTestId('scope').textContent).toBe('none');
  });
});
