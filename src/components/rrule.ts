// Re-export shim. The pure RRULE parse/build logic moved into the shared
// `@aperio/shared` package so the mobile event recurrence selector reuses it
// verbatim. This file stays as the desktop's stable import path (`./rrule`) —
// existing imports (RecurrenceSelector + its test) resolve unchanged.
export * from '@aperio/shared';
