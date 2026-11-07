import { act, fireEvent, render, screen } from '@testing-library/react';
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
    expect(optionValues(select)).toEqual(['relative', 'at_start', 'absolute']);
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

/**
 * A reminder at the moment of the appointment (221). Every provider stores it
 * as zero minutes before, and so does Aperio; the editor shows it as its own
 * kind, because an amount field only takes whole numbers from 1 and the
 * browser would refuse to save a 0 in it (`min={1}` inside a form).
 */
describe('RemindersEditor — a reminder at the start', () => {
  const AT_START: Reminder = { kind: { type: 'relative', minutes_before: 0 }, sound: null };

  function editor(
    value: EditableReminder[],
    props: { mode?: 'event' | 'task'; allDay?: boolean } = {},
  ) {
    const changes: EditableReminder[][] = [];
    render(
      <RemindersEditor
        value={value}
        onChange={(next) => changes.push(next)}
        mode={props.mode ?? 'event'}
        allDay={props.allDay}
      />,
    );
    const kind = screen.getByRole('combobox', { name: /Typ/i }) as HTMLSelectElement;
    return { kind, changes };
  }

  const selectedLabel = (select: HTMLSelectElement) =>
    select.options[select.selectedIndex].textContent;

  it('shows zero minutes before as "at start", with no amount to fill in', () => {
    const { kind } = editor([AT_START]);
    expect(kind.value).toBe('at_start');
    expect(selectedLabel(kind)).toMatch(/^(Zu Beginn|At start)$/);
    expect(screen.queryByRole('spinbutton')).toBeNull();
  });

  it('names it after what the row belongs to', () => {
    const task = editor([AT_START], { mode: 'task' });
    expect(selectedLabel(task.kind)).toMatch(/^(Bei Fälligkeit|When due)$/);
  });

  it('says "on the day" for an all-day appointment', () => {
    const { kind } = editor([AT_START], { allDay: true });
    expect(selectedLabel(kind)).toMatch(/^(Am Tag selbst|On the day)$/);
  });

  it('stores the choice as zero minutes before', () => {
    const { kind, changes } = editor([RELATIVE]);
    fireEvent.change(kind, { target: { value: 'at_start' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative', minutes_before: 0 });
  });

  it('goes back to 15 minutes before', () => {
    const { kind, changes } = editor([AT_START]);
    fireEvent.change(kind, { target: { value: 'relative' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative', minutes_before: 15 });
  });

  it('never turns a typed fraction into zero minutes before', () => {
    // A pasted 0.5, or a comma typed before the first digit (15 -> 0.15), was
    // checked before it was cut to a whole number and came out as 0: "at
    // start", a kind the row never chose.
    const { changes } = editor([RELATIVE]);
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '0.5' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative', minutes_before: 1 });
  });
});
