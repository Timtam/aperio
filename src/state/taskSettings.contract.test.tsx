import { describe, expect, it, vi } from 'vitest';

vi.mock('../api/client', () => ({
  getUserPref: vi.fn(() => Promise.resolve(null)),
  setUserPref: vi.fn(() => Promise.resolve()),
}));
vi.mock('../../mobile/src/api/prefs', () => ({
  getUserPref: vi.fn(() => Promise.resolve(null)),
  setUserPref: vi.fn(() => Promise.resolve()),
  deleteUserPref: vi.fn(() => Promise.resolve()),
  getUserPrefJson: vi.fn(() => Promise.resolve(null)),
  setUserPrefJson: vi.fn(() => Promise.resolve()),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
}));

import contract from '../../crates/cal-core/tests/fixtures/taskSettings.json';
import {
  answerCountdownWrite,
  answerDayWindowWrite,
  answerEffective,
  answerOverrideUpdate,
  answerRead,
  type EffectiveInput,
  type Num,
  type OverrideUpdateInput,
  type ReadInput,
} from './taskSettings.contractSupport';

/**
 * The task settings, pinned as a table before they move.
 *
 * How each surface reads the stored task and calendar preferences, resolves a
 * list's effective settings, and normalises what it writes back. Today every
 * rule exists twice — the desktop `TaskCascadeProvider` and the mobile
 * `taskBehaviour.ts` — and this file runs BOTH on every case: a row where the
 * surfaces differ says so and carries the desktop's answer beside mobile's.
 * The Rust side will read the same file.
 *
 * What stays out, and why, is written in the fixture's `notInThisTable`.
 */
interface Row<I> {
  name: string;
  note: string;
  input: I;
  expect: unknown;
  desktop?: unknown;
}

/** A fixture section as rows; JSON infers a union of literal shapes. */
const rows = <I,>(section: unknown): Row<I>[] => section as Row<I>[];

describe('taskSettings contract', () => {
  // Anti-silence: named rows, not a count.
  it('still carries the rows the rules turn on', () => {
    const named = (section: { name: string }[], ...needed: string[]) => {
      const names = new Set(section.map((r) => r.name));
      for (const n of needed) expect(names, `fixture lost ${n}`).toContain(n);
    };
    named(
      contract.read,
      'an-on-knob-turns-off-only-on-a-literal-false',
      'countdown-days-trailing-garbage-is-ignored',
      'an-out-of-order-window-is-the-whole-day',
      'a-trigger-off-the-list-is-midnight',
      'a-bad-field-leaves-the-good-ones',
      'one-failed-read',
    );
    named(contract.effective, 'an-override-wins-per-field');
    named(contract.countdownWrite, 'infinity-is-written-as-the-default');
    named(contract.dayWindowWrite, 'an-edge-between-half-hours-snaps');
    named(contract.overrideUpdate, 'an-update-keeps-its-place', 'fields-are-written-in-a-fixed-order');
  });

  const replay = <I, T>(
    section: string,
    sectionRows: Row<I>[],
    answer: (input: I) => Promise<{ mobile: T; desktop: T }>,
  ) => {
    for (const row of sectionRows) {
      it(`${section}: ${row.name}`, async () => {
        const { mobile, desktop } = await answer(row.input);
        expect(mobile, `mobile — ${row.note}`).toEqual(row.expect);
        expect(desktop, `desktop — ${row.note}`).toEqual(row.desktop ?? row.expect);
      });
    }
  };

  replay('read', rows<ReadInput>(contract.read), answerRead);
  replay('effective', rows<EffectiveInput>(contract.effective), answerEffective);
  replay('countdown write', rows<{ value: Num }>(contract.countdownWrite), (i) =>
    answerCountdownWrite(i.value),
  );
  replay('day window write', rows<{ start: Num; end: Num }>(contract.dayWindowWrite), (i) =>
    answerDayWindowWrite(i.start, i.end),
  );
  replay('override update', rows<OverrideUpdateInput>(contract.overrideUpdate), answerOverrideUpdate);
});
