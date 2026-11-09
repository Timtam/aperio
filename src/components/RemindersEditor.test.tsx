import { useState } from 'react';
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
const announced = vi.hoisted(() => [] as string[]);
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (message: string) => announced.push(message),
}));

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

/**
 * A reminder after the start (TODO B7 222): its own kind, offered only where
 * it arrives — the calendar keeps it, or the row stays in Aperio — and a row
 * that has it elsewhere says it will not arrive.
 */
describe('RemindersEditor — a reminder after the start', () => {
  const AFTER: EditableReminder = { kind: { type: 'relative', minutes_before: -10 }, sound: null };

  function editor(
    value: EditableReminder[],
    props: { afterStartKept?: boolean; placement?: boolean; mode?: 'event' | 'task' } = {},
  ) {
    const changes: EditableReminder[][] = [];
    render(
      <RemindersEditor
        value={value}
        onChange={(next) => changes.push(next)}
        mode={props.mode ?? 'event'}
        afterStartKept={props.afterStartKept}
        placement={props.placement}
        placementSurface="event"
      />,
    );
    const kind = screen.getByRole('combobox', { name: /Typ|Type/ }) as HTMLSelectElement;
    return { kind, changes };
  }

  it('shows it as "after start" with its amount, where the calendar keeps it', () => {
    const { kind } = editor([AFTER], { afterStartKept: true });
    expect(kind.value).toBe('after_start');
    expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('10');
    expect(screen.queryByText(/kommt dort nicht an|will not arrive/)).toBeNull();
  });

  it('keeps the row after the start when its amount changes', () => {
    const { changes } = editor([AFTER], { afterStartKept: true });
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '30' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative', minutes_before: -30 });
  });

  it('offers it only where it arrives', () => {
    const before: EditableReminder = { kind: { type: 'relative', minutes_before: 15 }, sound: null };
    const kept = editor([before], { afterStartKept: true });
    expect(optionValues(kept.kind)).toContain('after_start');
    document.body.innerHTML = '';
    // A calendar that drops it: not on an attached row…
    const attached = editor([{ ...before, attach: true }], { placement: true });
    expect(optionValues(attached.kind)).not.toContain('after_start');
    document.body.innerHTML = '';
    // …but on one that stays in Aperio.
    const inAperio = editor([{ ...before, attach: false }], { placement: true });
    expect(optionValues(inAperio.kind)).toContain('after_start');
  });

  it('says a row will not arrive where the calendar drops it, and how to keep it', () => {
    editor([{ ...AFTER, attach: true }], { placement: true });
    expect(screen.getByText(/kommt dort nicht an|will not arrive/).textContent).toMatch(
      /Nur in Aperio|Only in Aperio/,
    );
  });

  it('shows one on a task, but offers none', () => {
    const { kind } = editor([AFTER], { mode: 'task', afterStartKept: true });
    expect(kind.options[kind.selectedIndex].textContent).toMatch(/^(Nach Fälligkeit|After due)$/);
    document.body.innerHTML = '';
    const fresh = editor([{ kind: { type: 'relative', minutes_before: 15 }, sound: null }], {
      mode: 'task',
      afterStartKept: true,
    });
    expect(optionValues(fresh.kind)).not.toContain('after_start');
  });
});

/**
 * An all-day appointment's row is a day and a clock time (TODO B7 222), as
 * the planner rings its own reminders: the iPhone's "On day of event (9 AM)"
 * shows as the day itself at 09:00.
 */
describe('RemindersEditor — an all-day appointment', () => {
  function editor(value: EditableReminder[], props: { afterStartKept?: boolean } = {}) {
    const changes: EditableReminder[][] = [];
    render(
      <RemindersEditor
        value={value}
        onChange={(next) => changes.push(next)}
        mode="event"
        allDay
        afterStartKept={props.afterStartKept}
      />,
    );
    const kind = screen.getByRole('combobox', { name: /Typ|Type/ }) as HTMLSelectElement;
    return { kind, changes };
  }
  const rel = (minutes_before: number): EditableReminder => ({
    kind: { type: 'relative', minutes_before },
    sound: null,
  });
  const clockInput = () => screen.getByLabelText(/Eigene Uhrzeit|Own time/) as HTMLInputElement;
  const timeSelect = () =>
    screen.getByRole('combobox', { name: /^(Uhrzeit|Time)$/ }) as HTMLSelectElement;

  it("shows the iPhone's on-the-day alert as the day itself at 09:00", () => {
    const { kind } = editor([rel(-540)], { afterStartKept: true });
    expect(kind.value).toBe('on_day');
    expect(timeSelect().value).toBe('clock');
    expect(clockInput().value).toBe('09:00');
  });

  it('shows "1 day before (9 AM)" as one day before at 09:00, and the evening hour too', () => {
    const { kind } = editor([rel(900)]);
    expect(kind.value).toBe('days_before');
    expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('1');
    expect(clockInput().value).toBe('09:00');
    document.body.innerHTML = '';
    editor([rel(60)]);
    expect(clockInput().value).toBe('23:00');
  });

  it('sets a clock time the day before as minutes before midnight', () => {
    const { changes } = editor([rel(1440)]);
    expect(timeSelect().value).toBe('day_start');
    fireEvent.change(timeSelect(), { target: { value: 'clock' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative', minutes_before: 900 });
  });

  it('keeps the clock time when the number of days changes', () => {
    const { changes } = editor([rel(900)]);
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative', minutes_before: 2340 });
  });

  it('offers a clock time on the day itself, and days after, only where it arrives', () => {
    const { kind } = editor([rel(0)]);
    expect(screen.queryByRole('combobox', { name: /^(Uhrzeit|Time)$/ })).toBeNull();
    expect(optionValues(kind)).not.toContain('days_after');
    document.body.innerHTML = '';
    const kept = editor([rel(0)], { afterStartKept: true });
    expect(timeSelect().value).toBe('day_start');
    expect(optionValues(kept.kind)).toContain('days_after');
  });

  it('adds a row on the day itself at the day-start time', () => {
    const changes: EditableReminder[][] = [];
    render(
      <RemindersEditor value={[]} onChange={(next) => changes.push(next)} mode="event" allDay />,
    );
    act(() => {
      screen.getByRole('button', { name: /Erinnerung hinzufügen|Add reminder/i }).click();
    });
    expect(changes[0][0].kind).toEqual({ type: 'relative', minutes_before: 0 });
  });
});

/**
 * The fields of an all-day row as a keyboard and a screen reader meet them,
 * with the editor fed its own changes back, as the dialogs do.
 */
describe('RemindersEditor — editing an all-day row by keyboard', () => {
  function Harness({ start, afterStartKept = false }: { start: number; afterStartKept?: boolean }) {
    const [rows, setRows] = useState<EditableReminder[]>([
      { kind: { type: 'relative', minutes_before: start }, sound: null, attach: true },
    ]);
    return (
      <>
        <RemindersEditor
          value={rows}
          onChange={setRows}
          mode="event"
          allDay
          afterStartKept={afterStartKept}
          placement
          placementSurface="event"
        />
        <output data-testid="minutes">
          {rows[0].kind.type === 'relative' ? rows[0].kind.minutes_before : ''}
        </output>
      </>
    );
  }
  const minutes = () => screen.getByTestId('minutes').textContent;
  const clockInput = () => screen.getByLabelText(/Eigene Uhrzeit|Own time/) as HTMLInputElement;

  it('keeps the own-time field when a typed hour passes through 00:00', () => {
    // An hour typed with its leading zero ("0", then "8") is 00:00 for a moment.
    // That is the whole day, and so stored, but the focused field must stay.
    render(<Harness start={900} />);
    fireEvent.change(clockInput(), { target: { value: '00:00' } });
    expect(minutes()).toBe('1440');
    expect(clockInput().value).toBe('00:00');
    fireEvent.change(clockInput(), { target: { value: '08:00' } });
    expect(minutes()).toBe(String(1440 - 480));
  });

  it('keeps the time choice when the day-start time is chosen on the day itself', () => {
    // "On the day, 09:00" on a calendar that drops it: the time choice is only
    // there because the row has a time. Choosing the day-start time must not
    // take away the control that has focus.
    render(<Harness start={-540} />);
    const time = screen.getByRole('combobox', { name: /^(Uhrzeit|Time)$/ });
    fireEvent.change(time, { target: { value: 'day_start' } });
    expect(minutes()).toBe('0');
    expect(screen.getByRole('combobox', { name: /^(Uhrzeit|Time)$/ })).toBeInTheDocument();
  });

  it('says the hint when it appears, not when the row opens', () => {
    announced.length = 0;
    render(<Harness start={-540} />);
    expect(announced).toEqual([]);
    const applies = screen.getByRole('combobox', { name: /Gilt|Applies/ });
    fireEvent.change(applies, { target: { value: 'local' } });
    fireEvent.change(applies, { target: { value: 'attach' } });
    expect(announced).toHaveLength(1);
    expect(announced[0]).toMatch(/kommt dort nicht an|will not arrive/);
  });

  it('ties the "will not arrive" hint to the row\'s selects', () => {
    render(<Harness start={-540} />);
    const hint = screen.getByText(/kommt dort nicht an|will not arrive/);
    expect(hint.id).not.toBe('');
    const kind = screen.getByRole('combobox', { name: /Typ|Type/ });
    const applies = screen.getByRole('combobox', { name: /Gilt|Applies/ });
    expect(kind.getAttribute('aria-describedby')).toBe(hint.id);
    expect(applies.getAttribute('aria-describedby')).toBe(hint.id);
    // In Aperio it arrives: no hint, and nothing points at one.
    fireEvent.change(applies, { target: { value: 'local' } });
    expect(screen.queryByText(/kommt dort nicht an|will not arrive/)).toBeNull();
    expect(screen.getByRole('combobox', { name: /Typ|Type/ }).getAttribute('aria-describedby')).toBeNull();
  });
});

/**
 * A reminder counted from the end (CalDAV `TRIGGER;RELATED=END`, decided
 * 2026-11-09): its own kinds, offered only where they arrive — the calendar
 * keeps them, or the row stays in Aperio — and a row that has one elsewhere
 * says it will not arrive. A task has no end.
 */
describe('RemindersEditor — a reminder counted from the end', () => {
  const BEFORE_END: EditableReminder = {
    kind: { type: 'relative_to_end', minutes_before: 5 },
    sound: null,
  };

  function editor(
    value: EditableReminder[],
    props: {
      beforeEndKept?: boolean;
      placement?: boolean;
      mode?: 'event' | 'task';
      allDay?: boolean;
    } = {},
  ) {
    const changes: EditableReminder[][] = [];
    render(
      <RemindersEditor
        value={value}
        onChange={(next) => changes.push(next)}
        mode={props.mode ?? 'event'}
        allDay={props.allDay}
        beforeEndKept={props.beforeEndKept}
        placement={props.placement}
        placementSurface="event"
      />,
    );
    const kind = screen.getByRole('combobox', { name: /Typ|Type/ }) as HTMLSelectElement;
    return { kind, changes };
  }

  it('shows it as "before the end" with its amount, where the calendar keeps it', () => {
    const { kind } = editor([BEFORE_END], { beforeEndKept: true });
    expect(kind.value).toBe('before_end');
    expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('5');
    expect(screen.queryByText(/kommt dort nicht an|will not arrive/)).toBeNull();
  });

  it('keeps the row counted from the end when its amount changes', () => {
    const { changes } = editor([BEFORE_END], { beforeEndKept: true });
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '30' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative_to_end', minutes_before: 30 });
  });

  it('offers "before the end" and "at the end" only where they arrive', () => {
    const before: EditableReminder = { kind: { type: 'relative', minutes_before: 15 }, sound: null };
    const kept = editor([before], { beforeEndKept: true });
    expect(optionValues(kept.kind)).toEqual(
      expect.arrayContaining(['before_end', 'at_end']),
    );
    expect(optionValues(kept.kind)).not.toContain('after_end');
    document.body.innerHTML = '';
    const attached = editor([{ ...before, attach: true }], { placement: true });
    expect(optionValues(attached.kind)).not.toContain('before_end');
    document.body.innerHTML = '';
    const inAperio = editor([{ ...before, attach: false }], { placement: true });
    expect(optionValues(inAperio.kind)).toContain('before_end');
  });

  it('says a row will not arrive where the calendar counts from the start', () => {
    editor([{ ...BEFORE_END, attach: true }], { placement: true });
    expect(screen.getByText(/kommt dort nicht an|will not arrive/).textContent).toMatch(
      /ab dem Beginn|from the start/,
    );
  });

  it('offers none on a task', () => {
    const { kind } = editor([{ kind: { type: 'relative', minutes_before: 15 }, sound: null }], {
      mode: 'task',
      beforeEndKept: true,
    });
    expect(optionValues(kind)).not.toContain('before_end');
  });

  it('shows an all-day row after the end with its amount, and keeps it after the end', () => {
    const { kind, changes } = editor(
      [{ kind: { type: 'relative_to_end', minutes_before: -60 }, sound: null }],
      { allDay: true, beforeEndKept: true },
    );
    expect(kind.value).toBe('after_end');
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2' } });
    expect(changes.at(-1)?.[0].kind).toEqual({ type: 'relative_to_end', minutes_before: -120 });
  });

  it('shows an all-day row at the end without an amount', () => {
    const { kind } = editor(
      [{ kind: { type: 'relative_to_end', minutes_before: 0 }, sound: null }],
      { allDay: true, beforeEndKept: true },
    );
    expect(kind.value).toBe('at_end');
    expect(screen.queryByRole('spinbutton')).toBeNull();
  });

  it('shows one on an all-day appointment as what it is, but offers none there', () => {
    const { kind } = editor([BEFORE_END], { allDay: true, beforeEndKept: true });
    expect(kind.value).toBe('before_end');
    document.body.innerHTML = '';
    const fresh = editor([{ kind: { type: 'relative', minutes_before: 0 }, sound: null }], {
      allDay: true,
      beforeEndKept: true,
    });
    expect(optionValues(fresh.kind)).not.toContain('before_end');
  });
});
