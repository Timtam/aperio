import { describe, expect, it } from 'vitest';
import { nextPeriod, prevPeriod, visibleRange, today, VIEWS } from './viewMath';

const REF = new Date('2026-05-19T12:00:00Z');

describe('viewMath', () => {
  it('exposes the DESIGN.md views in stable order', () => {
    // Order matters: Ctrl+1..7 in `ViewState.tsx` is index-based, so a
    // shuffle here would silently re-map every shortcut. Contacts
    // joined the lineup in Phase 10a-3 (§10).
    expect(VIEWS).toEqual([
      'day',
      'week',
      'month',
      'year',
      'agenda',
      'tasks',
      'contacts',
    ]);
  });

  it('day view range covers the anchor day', () => {
    const { start, end } = visibleRange('day', REF);
    expect(start.getDate()).toBe(REF.getDate());
    expect(end.getTime() - start.getTime()).toBeGreaterThan(23 * 60 * 60 * 1000);
  });

  it('week view starts on Monday (ISO 8601) by default', () => {
    // 2026-05-19 is a Tuesday → range starts Monday 2026-05-18.
    const { start } = visibleRange('week', REF);
    expect(start.getDay()).toBe(1);
  });

  it('week view honours a configurable week start', () => {
    // getDay(): 0 = Sunday … 6 = Saturday — matches the weekStartsOn arg.
    expect(visibleRange('week', REF, 0).start.getDay()).toBe(0); // Sunday
    expect(visibleRange('week', REF, 1).start.getDay()).toBe(1); // Monday
    expect(visibleRange('week', REF, 6).start.getDay()).toBe(6); // Saturday
  });

  it('month view range covers the calendar month and the grid around it', () => {
    // This used to assert the range STOPPED at the month's own edges, which
    // is what left the grid's padding days empty — the view draws whole
    // weeks. The month itself must still be fully inside it.
    const { start, end } = visibleRange('month', REF);
    expect(start <= new Date(REF.getFullYear(), REF.getMonth(), 1)).toBe(true);
    expect(end >= new Date(REF.getFullYear(), REF.getMonth() + 1, 0)).toBe(true);
  });

  it('year view range covers the calendar year', () => {
    const { start, end } = visibleRange('year', REF);
    expect(start.getMonth()).toBe(0);
    expect(end.getMonth()).toBe(11);
  });

  it('nextPeriod / prevPeriod move by the right unit per view', () => {
    expect(prevPeriod('day', REF).getDate()).toBe(REF.getDate() - 1);
    expect(nextPeriod('day', REF).getDate()).toBe(REF.getDate() + 1);

    // Week step: +7 days.
    expect(nextPeriod('week', REF).getTime() - REF.getTime()).toBe(
      7 * 24 * 60 * 60 * 1000,
    );

    // Month step.
    expect(nextPeriod('month', REF).getMonth()).toBe(
      (REF.getMonth() + 1) % 12,
    );

    // Year step.
    expect(nextPeriod('year', REF).getFullYear()).toBe(REF.getFullYear() + 1);

    // Agenda + tasks step by month too.
    expect(nextPeriod('agenda', REF).getMonth()).toBe(
      (REF.getMonth() + 1) % 12,
    );
    expect(nextPeriod('tasks', REF).getMonth()).toBe(
      (REF.getMonth() + 1) % 12,
    );
  });

  it('today() returns midnight today', () => {
    const t = today();
    expect(t.getHours()).toBe(0);
    expect(t.getMinutes()).toBe(0);
    expect(t.getSeconds()).toBe(0);
    expect(t.getMilliseconds()).toBe(0);
  });
});

describe("visibleRange('month') covers the whole GRID", () => {
  it('reaches into the neighbouring months the grid actually draws', () => {
    // The month view draws whole weeks. Fetching only the month left the
    // padding days it renders permanently empty — an event on the 30th of
    // June was invisible to anyone looking at July's first row.
    // July 2026 starts on a Wednesday, so a Monday-start grid opens on
    // 29 June and closes on 2 August.
    const july = new Date(2026, 6, 15);
    const r = visibleRange('month', july, 1);
    expect(r.start.getMonth()).toBe(5); // June
    expect(r.start.getDate()).toBe(29);
    expect(r.end.getMonth()).toBe(7); // August
    expect(r.end.getDate()).toBe(2);
  });

  it('follows the week start the user chose', () => {
    const july = new Date(2026, 6, 15);
    expect(visibleRange('month', july, 1).start.getDay()).toBe(1); // Monday
    expect(visibleRange('month', july, 0).start.getDay()).toBe(0); // Sunday
  });

  it('still contains every day of the month itself', () => {
    for (const m of [0, 1, 5, 11]) {
      const r = visibleRange('month', new Date(2026, m, 15), 1);
      expect(r.start <= new Date(2026, m, 1)).toBe(true);
      expect(r.end >= new Date(2026, m + 1, 0, 23, 59)).toBe(true);
    }
  });
});
