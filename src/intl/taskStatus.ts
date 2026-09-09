// Re-export shim. The task status / priority / progress / assignee label
// helpers moved into the shared `@aperio/shared` package so the mobile app
// reuses them verbatim. This file stays as the desktop's stable import path
// (`../intl/taskStatus`) — existing imports resolve unchanged.
//
// It used to shadow three of those exports with the WebAssembly versions,
// because `@aperio/shared` still carried a TypeScript copy of the priority
// rules and only the desktop could reach Rust. Both halves of that are gone:
// the rule lives in `cal_core::task_priority`, and `@aperio/shared` reaches it
// through a door each surface installs at startup. There is one implementation
// now, so there is nothing left to shadow.
export * from '@aperio/shared';
