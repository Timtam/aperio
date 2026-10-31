import { describe, expect, it } from 'vitest';

import type { Section } from '@aperio/shared';

/**
 * The phone's section writes over its reads (review round 15 of #128), run
 * here because the phone has no test runner. Loaded through a path the type
 * checker does not follow: the desktop's tsc must not pull the phone's module
 * graph in (CI has no mobile packages).
 */
const MODULE = '../../mobile/src/state/sectionWrites';

type Writes = typeof import('../../mobile/src/state/sectionWrites');

async function load(): Promise<Writes> {
  return (await import(/* @vite-ignore */ MODULE)) as Writes;
}

const section = (id: string, name: string): Section => ({
  id,
  list_id: 'list-1',
  name,
  color_label: null,
  order: 0,
});

describe('section writes over a list\'s reads', () => {
  it('shows a created section over every read until one shows it', async () => {
    const { overRead, withWrite } = await load();
    const written = withWrite([], { upsert: section('sec-q4', 'Q4') });
    // The host serves the rows from before: the list editor opens again, a
    // refresh of another list reads every list.
    const first = overRead([section('sec-a', 'A')], written);
    expect(first.rows.map((s) => s.name)).toEqual(['A', 'Q4']);
    const again = overRead([section('sec-a', 'A')], first.open);
    expect(again.rows.map((s) => s.name)).toEqual(['A', 'Q4']);
    // The refresh landed.
    const landed = overRead([section('sec-a', 'A'), section('sec-q4', 'Q4')], again.open);
    expect(landed.open).toEqual([]);
    expect(landed.rows.map((s) => s.name)).toEqual(['A', 'Q4']);
  });

  it('keeps a deleted section out, and a later write of it takes its place', async () => {
    const { overRead, withWrite } = await load();
    const renamed = withWrite([], { upsert: section('sec-a', 'Neu') });
    const removed = withWrite(renamed, { remove: 'sec-a' });
    expect(removed).toEqual([{ remove: 'sec-a' }]);
    expect(overRead([section('sec-a', 'A')], removed).rows).toEqual([]);
    expect(overRead([], removed).open).toEqual([]);
  });
});
