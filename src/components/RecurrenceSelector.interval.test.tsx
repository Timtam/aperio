import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import { RecurrenceSelector } from './RecurrenceSelector';
// Side-effect import initialises the shared i18next instance so the labels
// resolve the dialogs.event.recurrence.* keys.
import '../i18n';

afterEach(() => {
  document.body.innerHTML = '';
});

// The interval field must let the user CLEAR it mid-edit. Binding straight to
// the number snapped an emptied field back to 1 — so changing "1" to "3" meant
// typing "3" onto the front ("13") and only then deleting the "1", which
// silently produced "every 13 weeks". The draft-text state fixes that.
describe('RecurrenceSelector interval field', () => {
  const renderSel = (onChange = vi.fn()) => {
    render(
      <RecurrenceSelector
        value="FREQ=WEEKLY;INTERVAL=1"
        onChange={onChange}
        startDay="2026-01-07"
      />,
    );
    return onChange;
  };

  const intervalInput = () => screen.getByRole('spinbutton') as HTMLInputElement;

  it('can be cleared to empty without snapping back to 1', () => {
    renderSel();
    const input = intervalInput();
    expect(input.value).toBe('1');
    fireEvent.change(input, { target: { value: '' } });
    // The field stays empty for the user to type the new number — it does NOT
    // reset to "1".
    expect(input.value).toBe('');
  });

  it('emits the new interval once a valid number is typed', () => {
    const onChange = renderSel();
    const input = intervalInput();
    fireEvent.change(input, { target: { value: '' } });
    fireEvent.change(input, { target: { value: '3' } });
    expect(input.value).toBe('3');
    // The last emitted rule carries INTERVAL=3 (not 13, not 1).
    const last = onChange.mock.calls.at(-1)?.[0] as string;
    expect(last).toContain('INTERVAL=3');
  });

  it('restores the last valid value on blur when left empty', () => {
    renderSel();
    const input = intervalInput();
    fireEvent.change(input, { target: { value: '' } });
    fireEvent.blur(input);
    expect(input.value).toBe('1');
  });
});

// Choosing "until a date" wrote no end while the field had no date, so the
// rule read back as "never" and the control jumped back at once.
describe('RecurrenceSelector end', () => {
  it('stays on "until a date" when chosen, starting on the tenth occurrence', () => {
    const onChange = vi.fn();
    render(<RecurrenceSelector value="FREQ=DAILY" onChange={onChange} startDay="2026-11-02" />);
    const endSelect = screen
      .getAllByRole('combobox')
      .find((el) => el.querySelector('option[value="UNTIL"]') !== null) as HTMLSelectElement;
    fireEvent.change(endSelect, { target: { value: 'UNTIL' } });
    expect(onChange).toHaveBeenLastCalledWith('FREQ=DAILY;UNTIL=20261111T235959Z');
  });
});

// WebView2 reports '' while a date input's segments name no date (a segment
// cleared, an invalid day on the way to another month). Bound to the rule,
// that wrote no end: the control read "never" and the field vanished under
// the keyboard focus.
describe('RecurrenceSelector end date', () => {
  const renderEnd = (onChange = vi.fn()) => {
    render(
      <RecurrenceSelector
        value="FREQ=DAILY;UNTIL=20261104T235958Z"
        onChange={onChange}
        startDay="2026-11-01"
      />,
    );
    return onChange;
  };
  const dateInput = () => document.querySelector('input[type="date"]') as HTMLInputElement;

  it('keeps the end and the field while the date is unfinished', () => {
    const onChange = renderEnd();
    fireEvent.change(dateInput(), { target: { value: '' } });
    expect(onChange).not.toHaveBeenCalled();
    expect(dateInput()).not.toBeNull();
    expect(dateInput().value).toBe('');
  });

  it('writes a whole date once it stands there', () => {
    const onChange = renderEnd();
    fireEvent.change(dateInput(), { target: { value: '' } });
    fireEvent.change(dateInput(), { target: { value: '2026-12-31' } });
    expect(onChange).toHaveBeenLastCalledWith('FREQ=DAILY;UNTIL=20261231T235959Z');
  });

  it('shows the end again when left unfinished', () => {
    renderEnd();
    fireEvent.change(dateInput(), { target: { value: '' } });
    fireEvent.blur(dateInput());
    expect(dateInput().value).toBe('2026-11-04');
  });
});

// Stage 12, part 3: a stored series' end field reads and writes its day on the
// series' clock (DESIGN-series-time-zone.md, 140a-142a), and says which clock.
describe("RecurrenceSelector end on a stored series' clock", () => {
  const NY = {
    start: '2026-11-03T00:00:00Z',
    allDay: false,
    tzid: 'America/New_York',
    deviceZone: 'Europe/Berlin',
  };
  const NO_START = { ...NY, start: '' };
  const VALUE = 'FREQ=DAILY;UNTIL=20261111T045959Z';

  it('names the clock and shows the day the end reaches there', () => {
    render(
      <RecurrenceSelector value={VALUE} onChange={vi.fn()} startDay="2026-11-02" endFacts={NY} clockName="New York" />,
    );
    const input = screen.getByLabelText('Ende-Datum (New York)') as HTMLInputElement;
    expect(input.value).toBe('2026-11-10');
  });

  it('keeps the plain label where the fields stand on the device clock', () => {
    render(<RecurrenceSelector value={VALUE} onChange={vi.fn()} startDay="2026-11-02" endFacts={NY} />);
    expect((screen.getByLabelText('Ende-Datum') as HTMLInputElement).value).toBe('2026-11-10');
  });

  it("spells a typed day on the series' clock", () => {
    const onChange = vi.fn();
    render(
      <RecurrenceSelector value={VALUE} onChange={onChange} startDay="2026-11-02" endFacts={NY} clockName="New York" />,
    );
    fireEvent.change(screen.getByLabelText('Ende-Datum (New York)'), { target: { value: '2026-11-12' } });
    expect(onChange).toHaveBeenLastCalledWith('FREQ=DAILY;UNTIL=20261113T045959Z');
  });

  it('writes nothing and says so where the day cannot be spelled (D6)', () => {
    const onChange = vi.fn();
    render(<RecurrenceSelector value={VALUE} onChange={onChange} startDay="2026-11-02" endFacts={NO_START} />);
    fireEvent.change(screen.getByLabelText('Ende-Datum'), { target: { value: '2026-11-12' } });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByText(/^Das Ende-Datum lässt sich auf diesem Gerät nicht schreiben\. /)).toBeTruthy();
  });
});
