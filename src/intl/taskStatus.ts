// Re-export shim. The task status / priority / progress / assignee label
// helpers moved into the shared `@aperio/shared` package so the mobile app
// reuses them verbatim. This file stays as the desktop's stable import path
// (`../intl/taskStatus`) — existing imports resolve unchanged.
export * from '@aperio/shared';

// …except for the three rules the desktop now gets from `cal-core` itself,
// compiled into this webview as WebAssembly. An explicit export shadows the
// star above, so every call site through this path switches at once without
// touching a line.
//
// These three were chosen for having no policy in them — no i18n keys, no
// glyphs — so that moving them decides nothing that is still open. And
// `priorityRank` in particular is the one that had to work: its callers are
// `Array.prototype.sort` comparators (`src/components/BacklogRail.tsx`), which
// are synchronous by construction and could never have awaited an `invoke`.
//
// The TypeScript originals are still in `@aperio/shared`, because
// `shared/taskGrouping.ts` calls `priorityRank` from inside the package and
// the mobile app runs that code too. That is a transitional state, not the end
// one: the TS copy goes when mobile gets the same door — its Expo module
// already exposes a synchronous `Function(...)`, so the road exists. Until
// then `src/wasm/coreRules.parity.test.ts` proves the two answer the same for
// every input there is.
export {
  isImportantPriority,
  normalPriority,
  priorityRank,
} from '../wasm/coreRules';
