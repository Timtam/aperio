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

  it('week view starts on Monday (ISO 8601)', () => {
    // 2026-05-19 is a Tuesday → range starts Monday 2026-05-18.
    const { start } = visibleRange('week', REF);
    expect(start.getDay()).toBe(1);
  });

  it('month view range covers the calendar month', () => {
    const { start, end } = visibleRange('month', REF);
    expect(start.getDate()).toBe(1);
    expect(end.getMonth()).toBe(REF.getMonth());
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
