//! Schema versioning + version-gating helpers (DESIGN.md §19.13,
//! Phase Sl).
//!
//! The sync dataset's `meta.json` carries two version fields:
//!
//!   - `schema_version` (u32) — bumped only on breaking changes
//!     to the log / snapshot / meta wire format.
//!   - `min_app_version` (semver string) — the minimum Aperio
//!     version that can safely read the dataset. Bumped in
//!     lockstep with `schema_version` whenever a forward-only
//!     change lands.
//!
//! The sync engine probes both before any data operation:
//!
//! ```text
//! Read meta.json
//!     │
//!     ▼
//! app_version < min_app_version?
//!     │              │
//!    Yes            No → continue
//!     │
//!     ▼
//! SyncError::SchemaTooOld → frontend shows the
//!                           "update required" modal
//! ```
//!
//! ## Why our own semver parser
//!
//! Aperio uses `CARGO_PKG_VERSION` (which is well-formed semver
//! by definition) and the `meta.json.min_app_version` is also
//! emitted by Aperio itself. The full RFC compliance the
//! `semver` crate offers (build metadata, complex pre-release
//! ordering) isn't worth the dep weight; a tuple-compare on the
//! first three numeric components catches every meaningful
//! ordering case for our datasets.

use serde::{Deserialize, Serialize};

use crate::error::{SyncError, SyncResult};

/// Outcome of comparing the running app's version against a
/// dataset's `min_app_version` + the schema versions.
///
/// `Ok` is the steady-state path; the other variants are
/// surfaced to the frontend so it can render the §19.13 update
/// modal or a forward-warning chip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Compatibility {
    /// The running version is at least `min_app_version` and the
    /// remote schema version is one this build understands. Sync
    /// proceeds normally.
    Ok,
    /// The running app is older than the dataset requires —
    /// `running_app_version < remote.min_app_version`. The user
    /// must update before the sync engine touches anything.
    AppTooOld {
        required: String,
        running: String,
    },
    /// The remote `schema_version` is newer than this build's
    /// `SCHEMA_VERSION`. Different from `AppTooOld` because it
    /// can occur even when the running version satisfies
    /// `min_app_version` — a future Aperio writer can bump the
    /// `schema_version` without immediately bumping
    /// `min_app_version` (the latter only goes up on breaking
    /// changes). Treated as a strict warning: we proceed because
    /// the spec promises additive changes are
    /// backward-compatible, but the frontend can show a chip.
    SchemaAhead {
        remote: u32,
        local: u32,
    },
}

impl Compatibility {
    /// `true` for everything except `Ok`.
    pub fn is_incompatible(&self) -> bool {
        !matches!(self, Compatibility::Ok)
    }

    /// `true` only for the hard-blocking case.
    pub fn is_blocking(&self) -> bool {
        matches!(self, Compatibility::AppTooOld { .. })
    }
}

/// Compare the running app version against the dataset's
/// requirements. Returns `Compatibility::Ok` when everything
/// lines up, the right warning variant otherwise.
pub fn check_compatibility(
    remote_schema: u32,
    remote_min_app_version: &str,
    running_app_version: &str,
    our_schema: u32,
) -> Compatibility {
    if version_less(running_app_version, remote_min_app_version) {
        return Compatibility::AppTooOld {
            required: remote_min_app_version.to_string(),
            running: running_app_version.to_string(),
        };
    }
    if remote_schema > our_schema {
        return Compatibility::SchemaAhead {
            remote: remote_schema,
            local: our_schema,
        };
    }
    Compatibility::Ok
}

/// Strict "less than" on the first three numeric components of a
/// semver-shaped string. Anything beyond the third dot (build /
/// pre-release suffix) is ignored.
///
/// Examples:
/// - `version_less("1.0.4", "1.2.0")` → `true`
/// - `version_less("1.2.0", "1.2.0")` → `false` (equal)
/// - `version_less("1.2.1", "1.2.0")` → `false`
/// - `version_less("1.10.0", "1.9.0")` → `false` (10 > 9)
/// - `version_less("2.0.0-beta", "2.0.0")` → `false` (we ignore the suffix)
///
/// Returns `false` on either side being unparseable — a malformed
/// `min_app_version` shouldn't lock the user out. The caller logs
/// at the call site.
pub fn version_less(a: &str, b: &str) -> bool {
    let Some(va) = parse_triple(a) else {
        return false;
    };
    let Some(vb) = parse_triple(b) else {
        return false;
    };
    va < vb
}

fn parse_triple(s: &str) -> Option<(u32, u32, u32)> {
    // Trim any pre-release / build metadata suffix.
    let trimmed = s
        .split(|c: char| c == '-' || c == '+')
        .next()
        .unwrap_or("");
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// Convenience for the orchestrator + onboarding paths: take a
/// `MetaJson` + the running app's version, return a `SyncResult`.
/// `AppTooOld` collapses into [`SyncError::SchemaTooOld`] so the
/// command layer can pattern-match on the error code.
pub fn ensure_compatible(
    meta: &crate::meta::MetaJson,
    running_app_version: &str,
) -> SyncResult<Compatibility> {
    let result = check_compatibility(
        meta.schema_version,
        &meta.min_app_version,
        running_app_version,
        crate::meta::SCHEMA_VERSION,
    );
    if let Compatibility::AppTooOld { required, running } = &result {
        return Err(SyncError::SchemaTooOld {
            required: required.clone(),
            running: running.clone(),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_less_basic_ordering() {
        assert!(version_less("1.0.4", "1.2.0"));
        assert!(!version_less("1.2.0", "1.0.4"));
        assert!(!version_less("1.2.0", "1.2.0"));
    }

    #[test]
    fn version_less_handles_double_digit_minor() {
        // The "1.10.0 vs 1.9.0" trap: string compare would put
        // 1.10 < 1.9. Numeric tuple compare gets it right.
        assert!(!version_less("1.10.0", "1.9.0"));
        assert!(version_less("1.9.0", "1.10.0"));
    }

    #[test]
    fn version_less_strips_prerelease_suffix() {
        // 2.0.0-beta and 2.0.0 sort as equal under our parser —
        // that's fine because Aperio doesn't gate behaviour on
        // pre-release ordering.
        assert!(!version_less("2.0.0-beta", "2.0.0"));
        assert!(!version_less("2.0.0", "2.0.0-beta"));
    }

    #[test]
    fn version_less_short_strings_zero_pad() {
        // "1" gets read as 1.0.0; "1.2" as 1.2.0.
        assert!(version_less("1", "1.0.1"));
        assert!(version_less("1.2", "1.2.1"));
        assert!(!version_less("1.2", "1.2"));
    }

    #[test]
    fn version_less_unparseable_returns_false() {
        assert!(!version_less("not-a-version", "1.0.0"));
        assert!(!version_less("1.0.0", "not-a-version"));
        assert!(!version_less("", ""));
    }

    #[test]
    fn check_compatibility_ok_when_app_at_or_above_min() {
        let c = check_compatibility(1, "1.0.0", "1.0.0", 1);
        assert_eq!(c, Compatibility::Ok);
        let c = check_compatibility(1, "1.0.0", "1.5.0", 1);
        assert_eq!(c, Compatibility::Ok);
    }

    #[test]
    fn check_compatibility_app_too_old() {
        let c = check_compatibility(2, "1.5.0", "1.0.0", 1);
        assert_eq!(
            c,
            Compatibility::AppTooOld {
                required: "1.5.0".into(),
                running: "1.0.0".into(),
            },
        );
        assert!(c.is_blocking());
        assert!(c.is_incompatible());
    }

    #[test]
    fn check_compatibility_schema_ahead_warns_but_doesnt_block() {
        // App satisfies min_app_version but the dataset has a
        // newer schema version (additive bump). Not blocking;
        // surface as `SchemaAhead` so the UI can show a chip.
        let c = check_compatibility(2, "1.0.0", "1.0.0", 1);
        assert_eq!(
            c,
            Compatibility::SchemaAhead {
                remote: 2,
                local: 1,
            },
        );
        assert!(!c.is_blocking());
        assert!(c.is_incompatible());
    }

    #[test]
    fn ensure_compatible_returns_error_on_app_too_old() {
        let mut meta = crate::meta::MetaJson::fresh("2.0.0");
        meta.min_app_version = "2.0.0".into();
        let err = ensure_compatible(&meta, "1.5.0").unwrap_err();
        assert!(matches!(err, SyncError::SchemaTooOld { .. }));
    }

    #[test]
    fn ensure_compatible_returns_ok_warning_for_schema_ahead() {
        // App ≥ min_app_version, but remote schema_version is
        // ahead → return Ok(SchemaAhead) so the caller can keep
        // syncing while surfacing the warning to the user.
        let mut meta = crate::meta::MetaJson::fresh("1.0.0");
        meta.schema_version = 99;
        meta.min_app_version = "1.0.0".into();
        let result = ensure_compatible(&meta, "1.0.0").unwrap();
        assert!(matches!(result, Compatibility::SchemaAhead { .. }));
    }
}
