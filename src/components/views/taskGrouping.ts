// Re-export shim. The task grouping rules (Backlog / Zukünftig / per-list /
// per-section / Done) moved into the shared `@aperio/shared` package so the
// mobile app reuses them verbatim. This file stays as the desktop's stable
// import path (`../views/taskGrouping`) — existing imports resolve unchanged.
export * from '@aperio/shared';
