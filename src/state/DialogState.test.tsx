import { describe, expect, it } from 'vitest';
import { act, render, screen } from '@testing-library/react';

import { DialogStateProvider } from './DialogState';
import { useDialogState } from './dialogStateContext';
import type { CalendarEvent } from '../api/types';

// A synthetic expanded occurrence carries a `series_id` — that's what
// isExpandedOccurrence keys on, and what triggers the up-front scope prompt.
const occurrence = {
  id: 'evt-1@2026-07-20T08:30:00Z',
  series_id: 'evt-1',
  calendar_id: 'cal-1',
  title: 'Tabletten nehmen',
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

function click(label: string) {
  act(() => {
    screen.getByText(label).click();
  });
}

describe('DialogState recurring-edit scope prompt', () => {
  it('opening a recurring occurrence shows the scope prompt first', () => {
    render(
      <DialogStateProvider>
        <Probe />
      </DialogStateProvider>,
    );
    expect(screen.getByTestId('kind').textContent).toBe('none');
    click('open-occ');
    expect(screen.getByTestId('kind').textContent).toBe('eventEditScope');
  });

  it('choosing "series" hands off to the editor locked to the series scope', () => {
    render(
      <DialogStateProvider>
        <Probe />
      </DialogStateProvider>,
    );
    click('open-occ');
    click('choose-series');
    expect(screen.getByTestId('kind').textContent).toBe('event');
    expect(screen.getByTestId('scope').textContent).toBe('series');
  });

  it('choosing "this occurrence" hands off scoped to the occurrence', () => {
    render(
      <DialogStateProvider>
        <Probe />
      </DialogStateProvider>,
    );
    click('open-occ');
    click('choose-occ');
    expect(screen.getByTestId('kind').textContent).toBe('event');
    expect(screen.getByTestId('scope').textContent).toBe('occurrence');
  });

  it('cancelling the prompt opens no editor', () => {
    render(
      <DialogStateProvider>
        <Probe />
      </DialogStateProvider>,
    );
    click('open-occ');
    click('cancel');
    expect(screen.getByTestId('kind').textContent).toBe('none');
  });

  it('a non-recurring event opens the editor directly, no prompt', () => {
    render(
      <DialogStateProvider>
        <Probe />
      </DialogStateProvider>,
    );
    click('open-single');
    expect(screen.getByTestId('kind').textContent).toBe('event');
    // No prompt ⇒ no forced scope; the editor falls back to its own default.
    expect(screen.getByTestId('scope').textContent).toBe('none');
  });
});
