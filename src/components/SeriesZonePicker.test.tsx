import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

import { AnnouncerContext } from '../a11y/announcerContext';
import i18n from '../i18n';
import { SeriesZonePicker, type CalendarZones } from './SeriesZonePicker';

/**
 * The series time zone picker on the desktop (stage 11): the search field with
 * the first focus and the count line as its description, a marking the keys
 * move, a choice only of a marked row that can be chosen, the count said after
 * a pause and never over a choice (measured 2026-11-22).
 */

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = '';
});

const t = i18n.getFixedT(i18n.language);

function open({
  stored = null,
  deviceZone = 'Europe/Berlin',
  calendarZones = 'all',
}: { stored?: string | null; deviceZone?: string | null; calendarZones?: CalendarZones } = {}) {
  const announce = vi.fn();
  const onPick = vi.fn();
  const onCancel = vi.fn();
  const onSubmit = vi.fn((event: Event) => event.preventDefault());
  render(
    <AnnouncerContext.Provider value={{ announce }}>
      <form onSubmit={(event) => onSubmit(event.nativeEvent)}>
        <SeriesZonePicker
          isOpen
          startAt="2026-11-22T23:30:00Z"
          today="2026-11-22T12:00:00Z"
          stored={stored}
          deviceZone={deviceZone}
          calendarZones={calendarZones}
          onPick={onPick}
          onCancel={onCancel}
        />
      </form>
    </AnnouncerContext.Provider>,
  );
  const search = screen.getByRole('combobox', { name: t('dialogs.seriesTimeZone.searchLabel') });
  const options = () => screen.getAllByRole('option');
  const marked = () => {
    const id = search.getAttribute('aria-activedescendant');
    return id === null ? null : document.getElementById(id);
  };
  return { announce, onPick, onCancel, onSubmit, search, options, marked };
}

describe('SeriesZonePicker', () => {
  it('opens on the search field, described by the count line', () => {
    const { search } = open();
    expect(screen.getByRole('dialog')).toHaveAccessibleName(t('dialogs.seriesTimeZone.title'));
    expect(document.activeElement).toBe(search);
    expect(search).toHaveAccessibleDescription(t('dialogs.seriesTimeZone.groupCount', { count: 62 }));
    expect(screen.getByRole('listbox')).toHaveAccessibleName(t('dialogs.seriesTimeZone.listLabel'));
  });

  it('marks nothing at first, and Enter then chooses nothing', () => {
    const { search, onPick, marked } = open();
    expect(marked()).toBeNull();
    fireEvent.keyDown(search, { key: 'Enter' });
    expect(onPick).not.toHaveBeenCalled();
  });

  it('moves the marking by row, by ten, and to either end', () => {
    const { search, options, marked } = open();
    const all = options();
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    expect(marked()).toBe(all[0]);
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    expect(marked()).toBe(all[1]);
    fireEvent.keyDown(search, { key: 'PageDown' });
    expect(marked()).toBe(all[11]);
    fireEvent.keyDown(search, { key: 'PageUp' });
    expect(marked()).toBe(all[1]);
    fireEvent.keyDown(search, { key: 'ArrowUp' });
    fireEvent.keyDown(search, { key: 'ArrowUp' });
    expect(marked()).toBe(all[0]);
    fireEvent.keyDown(search, { key: 'End' });
    expect(marked()).toBe(all[all.length - 1]);
    fireEvent.keyDown(search, { key: 'Home' });
    expect(marked()).toBe(all[0]);
    expect(marked()).toHaveAttribute('aria-selected', 'true');
  });

  it("chooses the marked row: here the device's zone", () => {
    const { search, onPick } = open();
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    // fireEvent returns false when the handler prevented the default: the
    // Enter never reaches the editor's form as a submit.
    expect(fireEvent.keyDown(search, { key: 'Enter' })).toBe(false);
    expect(onPick).toHaveBeenCalledWith({ kind: 'zone', zone: 'Europe/Berlin' });
  });

  it('keeps Enter from the form even with nothing marked', () => {
    const { search, onPick } = open();
    expect(fireEvent.keyDown(search, { key: 'Enter' })).toBe(false);
    expect(onPick).not.toHaveBeenCalled();
  });

  it('chooses a row with the mouse', () => {
    const { onPick, options } = open();
    const london = options().find((option) => option.textContent?.includes('London, Dublin'));
    fireEvent.mouseDown(london!, { button: 2 });
    expect(onPick).not.toHaveBeenCalled();
    fireEvent.mouseDown(london!);
    expect(onPick).toHaveBeenCalledWith({ kind: 'zone', zone: 'Europe/London' });
  });

  it('does not choose a row it marks as refused', () => {
    const { search, onPick, marked } = open({ calendarZones: 'none' });
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    expect(marked()).toHaveAttribute('aria-disabled', 'true');
    fireEvent.keyDown(search, { key: 'Enter' });
    expect(onPick).not.toHaveBeenCalled();
  });

  it('starts the marking over when the search changes', () => {
    const { search, marked } = open();
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    fireEvent.change(search, { target: { value: '+2' } });
    expect(marked()).toBeNull();
    expect(screen.getAllByRole('option')).toHaveLength(8);
  });

  it('says the count once the typing pauses', () => {
    vi.useFakeTimers();
    const { search, announce } = open();
    fireEvent.change(search, { target: { value: '+2' } });
    act(() => vi.advanceTimersByTime(599));
    expect(announce).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(1));
    expect(announce).toHaveBeenCalledWith(t('dialogs.seriesTimeZone.groupCount', { count: 8 }));
  });

  it('never says the count over a choice made within the pause', () => {
    vi.useFakeTimers();
    const { search, announce, onPick } = open();
    fireEvent.change(search, { target: { value: 'vienna' } });
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    fireEvent.keyDown(search, { key: 'Enter' });
    expect(onPick).toHaveBeenCalledWith({ kind: 'zone', zone: 'Europe/Vienna' });
    act(() => vi.advanceTimersByTime(2000));
    expect(announce).not.toHaveBeenCalled();
  });

  it('closes on Escape without a choice', () => {
    const { onCancel, onPick } = open();
    fireEvent.keyDown(screen.getByRole('dialog').parentElement!, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalled();
    expect(onPick).not.toHaveBeenCalled();
  });
});
