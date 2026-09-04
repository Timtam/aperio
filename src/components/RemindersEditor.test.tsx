import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { Reminder } from '../api/types';
import { RemindersEditor, type EditableReminder } from './RemindersEditor';

/**
 * Which reminder kinds a surface may offer.
 *
 * "On next app start" is host-local by construction: no wire format carries
 * it, and the collector that fires it reads an ENTRY's own reminders from the
 * local store. As a calendar default it would therefore save and then do
 * nothing — for a screen-reader user, an inert control is worse than an absent
 * one. So the calendar-defaults surfaces don't offer it, while the event and
 * task dialogs, where it does fire, still do.
 */

vi.mock('./SoundPicker', () => ({ SoundPicker: () => null }));

const RELATIVE: Reminder = {
  kind: { type: 'relative', minutes_before: 15 },
  sound: null,
};
const APP_START: Reminder = { kind: { type: 'app_start' }, sound: null };

function show(value: EditableReminder[], allowAppStart: boolean) {
  render(
    <RemindersEditor
      value={value}
      onChange={() => {}}
      mode="event"
      allowAppStart={allowAppStart}
    />,
  );
  return screen.getByRole('combobox', { name: /Typ/i }) as HTMLSelectElement;
}

const optionValues = (select: HTMLSelectElement) =>
  Array.from(select.options).map((o) => o.value);

describe('RemindersEditor — the kinds a surface offers', () => {
  it('offers app start on an entry, where it fires', () => {
    const select = show([RELATIVE], true);
    expect(optionValues(select)).toContain('app_start');
  });

  it('withholds app start where it could never fire', () => {
    const select = show([RELATIVE], false);
    expect(optionValues(select)).toEqual(['relative', 'absolute']);
  });

  it('keeps a stored app-start row readable and changeable, and says why', () => {
    // A list written before the offer was withdrawn: dropping the option would
    // make the select show a kind the row does not have.
    const select = show([APP_START], false);
    expect(select.value).toBe('app_start');
    expect(optionValues(select)).toContain('app_start');
    expect(
      screen.getByText(/nicht als Kalender-Standard|not as a calendar default/i),
    ).toBeInTheDocument();
  });

  it('adds a relative row, never an app-start one', () => {
    const changes: EditableReminder[][] = [];
    render(
      <RemindersEditor
        value={[]}
        onChange={(next) => changes.push(next)}
        mode="event"
        allowAppStart={false}
      />,
    );
    act(() => {
      screen.getByRole('button', { name: /Erinnerung hinzufügen|Add reminder/i }).click();
    });
    expect(changes).toHaveLength(1);
    expect(changes[0][0].kind.type).toBe('relative');
  });
});
