// Re-export shim. `localDateKey` moved into the shared `@aperio/shared` package
// so the mobile Agenda reuses it verbatim. This file stays as the desktop's
// stable import path (`./dateKey`) — existing imports resolve unchanged.
export { localDateKey } from '@aperio/shared';
