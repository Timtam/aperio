// URL detection for plain-text descriptions.
//
// The implementation moved to the shared, platform-agnostic `@aperio/shared`
// (so the desktop and the mobile app detect links identically — the
// security-sensitive scheme allowlist must never drift). This re-export keeps
// the desktop's `../util/links` import path stable.

export { detectLinks, ALLOWED_LINK_SCHEMES, schemeOf } from '@aperio/shared';
export type { DetectedLink } from '@aperio/shared';
