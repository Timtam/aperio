//! ABI + semantic-version constants and helpers.
//!
//! The ABI version is a single integer that the plugin manifest +
//! the compiled plugin both stamp into their handshake. The semver
//! comparator below is tiny — we deliberately avoid the `semver`
//! crate dep because Aperio + its plugins only ever emit
//! `CARGO_PKG_VERSION`-shaped strings, and the comparison is a
//! straight tuple-compare on the first three numeric components.
//! Same reasoning as `sync_core::version` already in the codebase.
//!
//! Build metadata, pre-release tags, and other RFC corners aren't
//! relevant here: if a future Aperio wants them it'll switch to the
//! `semver` crate at that point. Until then this is a 30-line
//! standalone parser.

use crate::error::{PluginError, PluginResult};

/// ABI version Aperio currently speaks. Mirrors
/// `APERIO_PLUGIN_ABI_VERSION` in `aperio_plugin.h`. A plugin whose
/// `abi_version` field doesn't equal this is refused at load time
/// (see [`crate::PluginError::AbiMismatch`]).
///
/// Bump rules: any breaking change to the C-ABI surface (struct
/// layout, vtable contracts) increments this by 1 and ships with
/// release notes.
///
/// ## History
///
/// - **v1** — initial. One process-singleton instance per loaded
///   library; lifecycle was `init(config_json) -> int` +
///   `destroy()` on the descriptor.
/// - **v2** — instance handles. The descriptor's `init`/`destroy`
///   were dropped in favour of `open_instance(config_json) ->
///   OpenInstanceResult` + `close_instance(handle)`; every
///   vtable method takes the opaque handle as its first argument.
///   The change unblocks DESIGN.md §6.4 (multiple accounts per
///   adapter type) — a single loaded library can now back N
///   independent adapter instances.
/// - **v3** — videoconference reach. `VcVtable` gained two appended
///   slots, `resolve_meeting` (find a meeting by its join link, the
///   only identifier that reaches a calendar event) and
///   `list_meetings` (surface meetings that have no calendar entry
///   at all). Appending to an EXISTING vtable is what forces a bump:
///   the host has no per-vtable length, so a plugin built against the
///   shorter layout would be read past its end. In the same revision
///   the `delete_meeting` slot's argument changed shape in place,
///   from a bare `MeetingId` string to `{id, notify_attendees}` —
///   taking a meeting down is also a question about the people
///   invited to it. **UNRELEASED at the time of writing**, which is
///   the only reason an in-place wire change was permissible: no
///   plugin exists that speaks the earlier v3. Once v3 ships, the
///   next wire change to an existing slot takes v4.
///
///   Also in v3, and this one touches every plugin: there is now ONE
///   outer vtable, [`crate::vtables::AdapterVtable`], with a pointer
///   per feature family (calendar, tasks, contacts, sync,
///   videoconference) and null for the rest. It replaces the old
///   arrangement where the cast depended on `plugin_type` — a
///   three-pointer wrapper for a calendar adapter, a bare
///   `SyncVtable` for a sync adapter, a bare `VcVtable` for a
///   videoconference one. That made the type tag load-bearing for
///   memory safety, and it made "this provider is a calendar AND a
///   place to sync into" unrepresentable, since a plugin has exactly
///   one vtable slot. The four per-surface `plugin_type` tags
///   collapsed into `"adapter"` in the same move: what a plugin does
///   is its `capabilities` list, which the host now cross-checks
///   against the non-null pointers at load time
///   ([`crate::vtables::check_declared_surfaces`]).
///
///   And, none of it breaking: `vtable_version` is now
///   actually READ before the rest of a vtable is trusted
///   ([`crate::vtables::vtable_layout_ok`]); the manifest gained the
///   optional `adapter_kind`, `account` and `strings` blocks; the
///   videoconference payloads gained `#[serde(default)]` fields
///   (`NewMeeting`: `use_personal_room`, `attendees`,
///   `notify_attendees`; `Meeting`: `invitees`, `join_details`).
///
///   Two optional named exports also arrived in the same development
///   cycle — `aperio_plugin_set_host_channel` and
///   `aperio_plugin_strings` — but neither is gated on v3, and
///   `set_host_channel` in fact landed while the ABI was still 2. A
///   named export never moves this number: the host looks it up by
///   symbol and asks a plugin that lacks it to do less.
///
/// The plugin-facing mirror of this list is
/// `web/src/content/docs/plugins/abi-versions.md`. THIS is the
/// authoritative copy; if the two disagree, the page is the bug.
pub const ABI_VERSION: u32 = 3;

/// Three-component semantic version. Only the (major, minor, patch)
/// tuple is preserved — pre-release / build metadata gets dropped
/// at parse time because Aperio never produces them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parse `"1.2.3"` (extra `-pre+meta` suffixes are tolerated +
    /// stripped). Returns [`PluginError::Semver`] on anything that
    /// doesn't have at least one numeric component.
    pub fn parse(input: &str) -> PluginResult<Self> {
        let core = input.split(['-', '+']).next().unwrap_or(input);
        let mut parts = core.split('.');
        // .next().unwrap_or — empty string yields "" which then
        // fails the u64 parse below with a clear error.
        let major = parts
            .next()
            .unwrap_or_default()
            .parse::<u64>()
            .map_err(|e| PluginError::Semver {
                value: input.to_string(),
                reason: format!("major: {e}"),
            })?;
        let minor =
            parts
                .next()
                .unwrap_or("0")
                .parse::<u64>()
                .map_err(|e| PluginError::Semver {
                    value: input.to_string(),
                    reason: format!("minor: {e}"),
                })?;
        let patch =
            parts
                .next()
                .unwrap_or("0")
                .parse::<u64>()
                .map_err(|e| PluginError::Semver {
                    value: input.to_string(),
                    reason: format!("patch: {e}"),
                })?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// Render back to `"major.minor.patch"`. Round-trips with
    /// [`Version::parse`] for any input that didn't carry a
    /// pre-release / build-metadata suffix.
    pub fn to_string_basic(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Check whether `app_version` (the running Aperio) satisfies the
/// plugin manifest's `min_app_version`. The host calls this at load
/// time and refuses the plugin with [`PluginError::AppTooOld`] when
/// the running build is older — same UX shape as the sync engine's
/// schema-too-old gate (the user updates the app, the plugin file
/// stays on disk untouched).
pub fn check_min_app_version(min_app_version: &str, app_version: &str) -> PluginResult<()> {
    let required = Version::parse(min_app_version)?;
    let running = Version::parse(app_version)?;
    if running >= required {
        Ok(())
    } else {
        Err(PluginError::AppTooOld {
            required: required.to_string_basic(),
            running: running.to_string_basic(),
        })
    }
}

/// Check that the manifest's ABI version equals the host's. Strict
/// equality (not >=) because the ABI is a single hard contract;
/// going forwards or backwards both mean someone needs to update.
pub fn check_abi_version(manifest_abi: u32) -> PluginResult<()> {
    if manifest_abi == ABI_VERSION {
        Ok(())
    } else {
        Err(PluginError::AbiMismatch {
            host: ABI_VERSION,
            plugin: manifest_abi,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_triple() {
        let v = Version::parse("1.2.3").expect("parses");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn parse_strips_prerelease_and_metadata() {
        let v = Version::parse("0.1.0-alpha+build").expect("parses");
        assert_eq!(
            v,
            Version {
                major: 0,
                minor: 1,
                patch: 0
            }
        );
    }

    #[test]
    fn parse_fills_missing_components_with_zero() {
        let v = Version::parse("2").expect("parses");
        assert_eq!(
            v,
            Version {
                major: 2,
                minor: 0,
                patch: 0
            }
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = Version::parse("not-a-version").unwrap_err();
        match err {
            PluginError::Semver { .. } => {}
            other => panic!("expected Semver, got {other:?}"),
        }
    }

    #[test]
    fn version_ordering_is_tuple_compare() {
        assert!(Version::parse("1.2.3").unwrap() < Version::parse("1.2.4").unwrap());
        assert!(Version::parse("1.10.0").unwrap() > Version::parse("1.9.99").unwrap());
        assert!(Version::parse("2.0.0").unwrap() > Version::parse("1.99.99").unwrap());
    }

    #[test]
    fn min_app_version_ok_when_running_equal() {
        check_min_app_version("0.1.0", "0.1.0").expect("equal is ok");
    }

    #[test]
    fn min_app_version_ok_when_running_newer() {
        check_min_app_version("0.1.0", "1.2.3").expect("newer is ok");
    }

    #[test]
    fn min_app_version_rejects_older_running() {
        let err = check_min_app_version("1.0.0", "0.9.9").unwrap_err();
        match err {
            PluginError::AppTooOld { required, running } => {
                assert_eq!(required, "1.0.0");
                assert_eq!(running, "0.9.9");
            }
            other => panic!("expected AppTooOld, got {other:?}"),
        }
    }

    #[test]
    fn abi_check_accepts_current_version() {
        check_abi_version(ABI_VERSION).expect("matches");
    }

    #[test]
    fn abi_check_rejects_mismatched_version() {
        let err = check_abi_version(ABI_VERSION + 1).unwrap_err();
        match err {
            PluginError::AbiMismatch { host, plugin } => {
                assert_eq!(host, ABI_VERSION);
                assert_eq!(plugin, ABI_VERSION + 1);
            }
            other => panic!("expected AbiMismatch, got {other:?}"),
        }
    }
}
