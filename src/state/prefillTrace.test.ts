import { afterEach, describe, expect, it, vi } from 'vitest';

import { calendarPlace, tracePrefill } from './prefillTrace';

describe('the prefill trace', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('names a calendar by its place in the catalog, never by its id', () => {
    const catalog = [{ id: 'anna@example.org' }, { id: 'cal-work' }];
    expect(calendarPlace(catalog, 'cal-work')).toBe('#1');
    expect(calendarPlace(catalog, 'gone')).toBe('?');
    expect(calendarPlace(catalog, '')).toBe('-');
    expect(calendarPlace(catalog, null)).toBe('-');

    const info = vi.spyOn(console, 'info').mockImplementation(() => {});
    tracePrefill('quick-add accept', { offer: calendarPlace(catalog, 'anna@example.org') });
    const line = String(info.mock.calls[0][0]);
    expect(line).toMatch(/^\[prefill \+\d+ms\] quick-add accept: offer=#0$/);
    expect(line).not.toContain('example.org');
  });
});
