import { describe, expect, it } from 'vitest';

import {
  reconcileSelectionTracked,
  type SelectionSlice,
} from './selectionReconcile';

/** Minimal container: id + owning account. */
const item = (id: string, account_id = 'acc1') => ({ id, account_id });

const slice = (
  selected: string[],
  known: string[] | null,
  origin: Record<string, string> = {},
): SelectionSlice => ({
  selected: new Set(selected),
  known: known ? new Set(known) : null,
  origin,
});

const sel = (s: SelectionSlice) => [...s.selected].sort();
const known = (s: SelectionSlice) => [...(s.known ?? [])].sort();

describe('reconcileSelectionTracked', () => {
  it('first run selects everything and learns origins', () => {
    const next = reconcileSelectionTracked(slice([], null), [
      item('a'),
      item('b', 'acc2'),
    ]);
    expect(sel(next)).toEqual(['a', 'b']);
    expect(known(next)).toEqual(['a', 'b']);
    expect(next.origin).toEqual({ a: 'acc1', b: 'acc2' });
  });

  it('first run respects the autoSelectNew veto', () => {
    const next = reconcileSelectionTracked(
      slice([], null),
      [item('rw'), item('ro')],
      (x) => x.id !== 'ro',
    );
    expect(sel(next)).toEqual(['rw']);
    expect(known(next)).toEqual(['ro', 'rw']);
  });

  it('upgrade freezes known and keeps the selection verbatim', () => {
    // Pre-known-tracking blob: selection stays untouched even when an id
    // is missing from the (possibly cold) listing.
    const next = reconcileSelectionTracked(slice(['a', 'missing'], null), [
      item('a'),
      item('b'),
    ]);
    expect(sel(next)).toEqual(['a', 'missing']);
    expect(known(next)).toEqual(['a', 'b', 'missing']);
    // 'b' is known now, so it is NOT auto-selected later.
  });

  it('auto-selects a truly new id but not a known unticked one', () => {
    const prev = slice(['a'], ['a', 'unticked'], {
      a: 'acc1',
      unticked: 'acc1',
    });
    const next = reconcileSelectionTracked(prev, [
      item('a'),
      item('unticked'),
      item('fresh'),
    ]);
    expect(sel(next)).toEqual(['a', 'fresh']);
  });

  it('drops an id its own account no longer lists (genuine removal)', () => {
    const prev = slice(['a', 'gone'], ['a', 'gone'], {
      a: 'acc1',
      gone: 'acc1',
    });
    // acc1 answered WITH content (it lists 'a') but no longer 'gone'.
    const next = reconcileSelectionTracked(prev, [item('a')]);
    expect(sel(next)).toEqual(['a']);
    expect(known(next)).toEqual(['a']);
    expect(next.origin).toEqual({ a: 'acc1' });
  });

  it('retains ids of an account absent from a cold listing', () => {
    const prev = slice(['a', 'cold'], ['a', 'cold'], {
      a: 'acc1',
      cold: 'acc2',
    });
    // acc2 contributed nothing (cold snapshot / transient failure) — its
    // ids must survive so the selection (and every data-hook cache key
    // derived from it) stays stable across the warm-up.
    const next = reconcileSelectionTracked(prev, [item('a')]);
    expect(sel(next)).toEqual(['a', 'cold']);
    expect(known(next)).toEqual(['a', 'cold']);
    expect(next.origin).toEqual({ a: 'acc1', cold: 'acc2' });
  });

  it('does not re-select an unticked id when its account warms back up', () => {
    // The historical corruption: a cold listing used to TRIM `known`, so
    // the unticked id counted as never-seen when the account warmed and
    // was auto-selected against the user's explicit choice.
    const prev = slice(['a'], ['a', 'unticked'], {
      a: 'acc1',
      unticked: 'acc2',
    });
    // Round 1: acc2 cold — nothing changes.
    const mid = reconcileSelectionTracked(prev, [item('a')]);
    expect(sel(mid)).toEqual(['a']);
    expect(known(mid)).toEqual(['a', 'unticked']);
    // Round 2: acc2 warm again — 'unticked' is still known → stays off.
    const next = reconcileSelectionTracked(mid, [
      item('a'),
      item('unticked', 'acc2'),
    ]);
    expect(sel(next)).toEqual(['a']);
  });

  it('retains an id with no recorded origin (conservative default)', () => {
    const prev = slice(['a', 'legacy'], ['a', 'legacy'], { a: 'acc1' });
    const next = reconcileSelectionTracked(prev, [item('a')]);
    expect(sel(next)).toEqual(['a', 'legacy']);
  });
});
