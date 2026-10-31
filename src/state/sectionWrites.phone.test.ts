import { describe, expect, it } from 'vitest';

import type { Section } from '@aperio/shared';

/**
 * The phone's section writes over its reads (review round 15 of #128), run
 * here because the phone has no test runner. Loaded through a path the type
 * checker does not follow: the desktop's tsc must not pull the phone's module
 * graph in (CI has no mobile packages).
 */
const MODULE = '../../mobile/src/state/sectionWrites';

/** The phone module's shape, written out here: a `typeof import()` of it
 *  would pull it into the desktop's tsc after all (review round 21 of #128). */
interface SectionWrite {
  upsert?: Section;
  remove?: string;
  before?: readonly Section[];
}
interface Writes {
  withWrite(writes: readonly SectionWrite[], write: SectionWrite): SectionWrite[];
  overRead(
    read: readonly Section[],
    writes: readonly SectionWrite[],
  ): { rows: Section[]; open: SectionWrite[] };
}

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

  it('ends a write once the host serves anything else than before it', async () => {
    // Review round 16 of #128: a section another device deleted or renamed
    // before Aperio read its own write stayed in every picker.
    const { overRead, withWrite } = await load();
    const before = [section('sec-a', 'A')];
    const written = withWrite([], { upsert: section('sec-a', 'Neu'), before });
    expect(overRead(before, written).rows.map((s) => s.name)).toEqual(['Neu']);
    // The refresh landed: another device had renamed it once more.
    const moved = overRead([section('sec-a', 'Später')], written);
    expect(moved.open).toEqual([]);
    expect(moved.rows.map((s) => s.name)).toEqual(['Später']);
  });

  it('keeps a write open while only a colour of the rows from before changed', async () => {
    // Review round 17 of #128: a rename saved with a new colour showed the
    // old name again, as the colour, stored at once, made the read differ.
    const { overRead, withWrite } = await load();
    const before = [section('sec-a', 'Alt'), section('sec-b', 'B')];
    const written = withWrite([], { upsert: section('sec-a', 'Neu'), before });
    const coloured = [{ ...section('sec-a', 'Alt'), color_label: 'blue' }, section('sec-b', 'B')];
    const read = overRead(coloured, written);
    expect(read.open).toHaveLength(1);
    expect(read.rows.map((s) => s.name).sort()).toEqual(['B', 'Neu']);
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
