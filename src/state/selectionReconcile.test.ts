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

  it('an upgrade against a COLD listing must not freeze known on nothing', () => {
    // The reported bug. `known` is null (a blob from before it existed, or one
    // that never got written), the user has 'a' ticked and 'b' deliberately
    // unticked, and at startup the listing is still cold — every external
    // catalog is served from a snapshot that has not warmed yet.
    //
    // Freezing `known := selected ∪ []` here records "the only container that
    // ever existed is 'a'". When the real listing lands a beat later, 'b' has
    // never been seen — so it is auto-selected, and the user's untick is gone.
    // The cold-listing rule that guards the steady state has to guard this too.
    const cold = reconcileSelectionTracked(slice(['a'], null), []);
    const warm = reconcileSelectionTracked(cold, [item('a'), item('b')]);
    expect(sel(warm)).toEqual(['a']);
    expect(known(warm)).toEqual(['a', 'b']);
  });

  it('a first run against a COLD listing waits rather than learning nothing', () => {
    // Same hole from the other side: with nothing selected and nothing known,
    // an empty listing would set `known` to the empty SET — no longer null, so
    // the next listing takes the steady-state path and every container counts
    // as new. That happens to be right on a genuine first run, and wrong the
    // moment anything was ever unticked. Deferring is right in both.
    const cold = reconcileSelectionTracked(slice([], null), []);
    expect(cold.known).toBeNull();
    const warm = reconcileSelectionTracked(cold, [item('a'), item('b')]);
    expect(sel(warm)).toEqual(['a', 'b']);
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

  it('prunes ids of a DELETED account when the account set is known', () => {
    const prev = slice(['a', 'orphan'], ['a', 'orphan'], {
      a: 'acc1',
      orphan: 'acc2',
    });
    // acc2 was deleted: absent from the listing AND from the accounts
    // table — without this pruning its ids would be immortal.
    const next = reconcileSelectionTracked(
      prev,
      [item('a')],
      undefined,
      new Set(['local', 'acc1']),
    );
    expect(sel(next)).toEqual(['a']);
    expect(known(next)).toEqual(['a']);
  });

  it('keeps cold-account ids when the account still exists', () => {
    const prev = slice(['a', 'cold'], ['a', 'cold'], {
      a: 'acc1',
      cold: 'acc2',
    });
    const next = reconcileSelectionTracked(
      prev,
      [item('a')],
      undefined,
      new Set(['local', 'acc1', 'acc2']),
    );
    expect(sel(next)).toEqual(['a', 'cold']);
  });
});
