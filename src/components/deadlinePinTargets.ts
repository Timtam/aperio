// `filterDeadlinePinTargets` now lives in `@aperio/shared` (shared with the
// mobile day-start checks). This re-export keeps the existing desktop import
// path + tests stable.
export { filterDeadlinePinTargets } from '@aperio/shared';
