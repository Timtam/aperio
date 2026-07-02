import { describe, expect, it } from 'vitest';

import { selectableEventCalendars, selectableTaskLists } from '@aperio/shared';

const cals = [
  { id: 'writable-visible', read_only: false },
  { id: 'writable-hidden', read_only: false },
  { id: 'readonly-visible', read_only: true },
  { id: 'readonly-hidden', read_only: true },
];

describe('selectableEventCalendars', () => {
  it('keeps only writable + visible when a selection is given (desktop)', () => {
    const visible = new Set(['writable-visible', 'readonly-visible']);
    const ids = selectableEventCalendars(cals, { selectedIds: visible }).map(
      (c) => c.id,
    );
    expect(ids).toEqual(['writable-visible']);
  });

  it('keeps only writable when no selection is given (mobile)', () => {
    const ids = selectableEventCalendars(cals).map((c) => c.id);
    expect(ids).toEqual(['writable-visible', 'writable-hidden']);
  });

  it('always keeps the current calendar, even hidden + read-only', () => {
    const visible = new Set(['writable-visible']);
    const ids = selectableEventCalendars(cals, {
      selectedIds: visible,
      currentId: 'readonly-hidden',
    }).map((c) => c.id);
    expect(ids).toEqual(['writable-visible', 'readonly-hidden']);
  });

  it('does not duplicate the current calendar when it already qualifies', () => {
    const visible = new Set(['writable-visible']);
    const ids = selectableEventCalendars(cals, {
      selectedIds: visible,
      currentId: 'writable-visible',
    }).map((c) => c.id);
    expect(ids).toEqual(['writable-visible']);
  });
});

// The task twin shares the container filter — assert the same contract so a
// drift between the two exports can't go unnoticed.
describe('selectableTaskLists', () => {
  const lists = [
    { id: 'writable-checked', read_only: false },
    { id: 'writable-unchecked', read_only: false },
    { id: 'readonly-checked', read_only: true },
  ];

  it('keeps only writable + checked lists when a selection is given', () => {
    const checked = new Set(['writable-checked', 'readonly-checked']);
    const ids = selectableTaskLists(lists, { selectedIds: checked }).map(
      (l) => l.id,
    );
    expect(ids).toEqual(['writable-checked']);
  });

  it('keeps only writable lists when no selection is given', () => {
    const ids = selectableTaskLists(lists).map((l) => l.id);
    expect(ids).toEqual(['writable-checked', 'writable-unchecked']);
  });

  it('always keeps the current list, even unchecked + read-only', () => {
    const checked = new Set<string>();
    const ids = selectableTaskLists(lists, {
      selectedIds: checked,
      currentId: 'readonly-checked',
    }).map((l) => l.id);
    expect(ids).toEqual(['readonly-checked']);
  });
});
