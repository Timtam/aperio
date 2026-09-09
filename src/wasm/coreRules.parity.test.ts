import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeAll, describe, expect, it } from 'vitest';

import {
  isImportantPriority as tsIsImportantPriority,
  normalPriority as tsNormalPriority,
  priorityRank as tsPriorityRank,
} from '@aperio/shared';
import type { PriorityScale, TaskPriority } from '@aperio/shared';

import {
  initSync,
  isImportantPriority as wasmIsImportantPriority,
  normalPriority as wasmNormalPriority,
  priorityRank as wasmPriorityRank,
} from '../../crates/cal-core-wasm/pkg/cal_core_wasm';

/**
 * Does the Rust answer what the TypeScript answers — for every input there is?
 *
 * This is the probe's actual question. The mechanism working (a .wasm module
 * loads and a function returns a number) proves nothing on its own; what has to
 * hold is that moving a rule into the core does not quietly change an answer.
 *
 * The input space here is three priorities times two scales, so this is not a
 * sample — it is the whole domain, enumerated. That is the strongest form this
 * kind of check can take, and it is available precisely because these functions
 * were chosen for having no policy in them.
 *
 * The module is instantiated with `initSync` from bytes on disk. Browsers
 * refuse synchronous compilation of a module this size on the main thread —
 * which is why the app awaits `initCoreRules()` once at startup — but Node has
 * no such limit, so the test needs no async setup and no fetch.
 */

const PRIORITIES: TaskPriority[] = ['low', 'medium', 'high'];
const SCALES: PriorityScale[] = ['three', 'two'];

beforeAll(() => {
  // Resolved from the project root, which is where vitest runs. `import.meta.url`
  // is not a file: URL under vitest's transform, so it cannot be used here.
  const wasmPath = resolve(
    process.cwd(),
    'crates/cal-core-wasm/pkg/cal_core_wasm_bg.wasm',
  );
  initSync({ module: readFileSync(wasmPath) });
});

describe('cal-core in WebAssembly answers what shared/taskStatus.ts answers', () => {
  it('priorityRank agrees on every priority and every scale', () => {
    for (const scale of SCALES) {
      for (const priority of PRIORITIES) {
        expect(
          wasmPriorityRank(priority, scale),
          `priorityRank(${priority}, ${scale})`,
        ).toBe(tsPriorityRank(priority, scale));
      }
    }
  });

  it('priorityRank agrees on the omitted-scale default too', () => {
    // The TS signature defaults `scale` to 'three' because the comparators in
    // BacklogRail call it with one argument. The Rust side has no defaults, so
    // the loader supplies it — and that seam is exactly where a difference
    // would hide.
    for (const priority of PRIORITIES) {
      expect(wasmPriorityRank(priority, 'three')).toBe(tsPriorityRank(priority));
    }
  });

  it('isImportantPriority agrees on every priority', () => {
    for (const priority of PRIORITIES) {
      expect(wasmIsImportantPriority(priority), priority).toBe(
        tsIsImportantPriority(priority),
      );
    }
  });

  it('normalPriority agrees, including on nothing-before', () => {
    for (const previous of [...PRIORITIES, null, undefined]) {
      expect(
        wasmNormalPriority(previous ?? undefined),
        `normalPriority(${String(previous)})`,
      ).toBe(tsNormalPriority(previous));
    }
  });

  it('is usable inside a synchronous Array.sort comparator', () => {
    // The whole reason WebAssembly is on the table. A comparator cannot await;
    // an async one returns promises, every promise compares equal, and the list
    // comes out in arbitrary order. This is the shape BacklogRail.tsx:180 uses.
    const rows: { priority: TaskPriority }[] = [
      { priority: 'low' },
      { priority: 'high' },
      { priority: 'medium' },
    ];
    const sorted = [...rows].sort(
      (a, b) =>
        wasmPriorityRank(a.priority, 'three') - wasmPriorityRank(b.priority, 'three'),
    );
    expect(sorted.map((r) => r.priority)).toEqual(['high', 'medium', 'low']);
  });

  it('refuses a value nobody wrote instead of answering for it', () => {
    // The TS side cannot express this — its parameter type keeps the value out
    // at compile time and it has no runtime guard. Rust checks at the boundary,
    // which is a real difference and the right one: data crossing from
    // JavaScript is unvalidated by construction.
    expect(() => wasmPriorityRank('urgent', 'three')).toThrow(/unknown priority/);
    expect(() => wasmPriorityRank('high', 'four')).toThrow(/unknown priority scale/);
  });
});
