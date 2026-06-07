import { describe, expect, it } from 'vitest';

import type { Calendar, CalendarEvent, ColorLabel } from '../api/types';
import { resolveEventColor } from './eventColor';

function calendar(id: string, hex: string | null): Calendar {
  return {
    id,
    name: id,
    color: hex ? { hex, source: 'native' } : null,
    color_label: null,
    read_only: false,
    default_sound: null,
    account_id: 'acc',
  };
}

function event(partial: Partial<CalendarEvent>): CalendarEvent {
  return {
    id: 'e1',
    calendar_id: 'cal',
    title: 'T',
    description: null,
    location: null,
    start: '2026-06-01T09:00:00Z',
    end: '2026-06-01T10:00:00Z',
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
    created_at: '2026-06-01T09:00:00Z',
    updated_at: '2026-06-01T09:00:00Z',
    etag: null,
    ...partial,
  };
}

describe('resolveEventColor', () => {
  const cals = new Map([['cal', calendar('cal', '#0000ff')]]);
  const labels = new Map<string, ColorLabel>([
    ['lbl', { id: 'lbl', name: 'Work', hex: '#ff0000', ad_hoc: false }],
  ]);

  it('uses the event color label when present', () => {
    const r = resolveEventColor(event({ color_label: 'lbl' }), cals, labels);
    expect(r).toEqual({ hex: '#ff0000', labelName: 'Work' });
  });

  it('renders an unmapped native color_hex directly (unnamed)', () => {
    // A subscribed iCal feed's color, or a foreign CalDAV color, that the
    // host couldn't map to a known label.
    const r = resolveEventColor(
      event({ color_label: null, color_hex: '#abcdef' }),
      cals,
      labels,
    );
    expect(r).toEqual({ hex: '#abcdef', labelName: null });
  });

  it('prefers the named label over a native color_hex when both are set', () => {
    const r = resolveEventColor(
      event({ color_label: 'lbl', color_hex: '#abcdef' }),
      cals,
      labels,
    );
    expect(r).toEqual({ hex: '#ff0000', labelName: 'Work' });
  });

  it('falls back to the calendar color when neither label nor color_hex resolves', () => {
    const r = resolveEventColor(
      event({ color_label: null, color_hex: null }),
      cals,
      labels,
    );
    expect(r).toEqual({ hex: '#0000ff', labelName: null });
  });

  it('falls through an unknown label id to the native color_hex', () => {
    const r = resolveEventColor(
      event({ color_label: 'missing', color_hex: '#abcdef' }),
      cals,
      labels,
    );
    expect(r).toEqual({ hex: '#abcdef', labelName: null });
  });
});
