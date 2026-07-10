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
        start={new Date(2026, 0, 7)}
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
