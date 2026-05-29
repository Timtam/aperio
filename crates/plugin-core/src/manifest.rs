//! `plugin.json` parser — DESIGN.md §20.4.
//!
//! Every plugin (bundled or community) ships its descriptor as a
//! sibling `plugin.json` file next to the platform shared
//! libraries. The manager reads this BEFORE attempting to dlopen
//! the binary so an obvious mismatch (wrong ABI, app too old) is
//! caught without ever loading code into the process.
//!
//! Example (from DESIGN.md):
//!
//! ```json
//! {
//!   "id": "com.example.myplugin",
//!   "name": "Mein Kalender-Plugin",
//!   "version": "1.0.0",
//!   "plugin_type": "calendar-adapter",
//!   "capabilities": ["calendar"],
//!   "abi_version": 1,
//!   "min_app_version": "1.0.0",
//!   "author": "Max Mustermann",
//!   "description": "Verbindet sich mit XY-Kalender",
//!   "signed": false
//! }
//! ```
//!
//! Required fields: `id`, `name`, `version`, `plugin_type`,
//! `abi_version`, `min_app_version`. Everything else is optional —
//! plugins without capabilities (e.g. notification channels)
//! omit the `capabilities` array entirely.
//!
//! Plugin signing — per the project decision recorded at P0 plan
//! time — is intentionally NOT implemented in this phase. The
//! `signed` field is preserved through parse + serialise for
//! forward-compat (future Aperio versions might verify
//! cryptographic signatures), but no host code looks at the value.
//! Every plugin is treated as unsigned and the install dialog
//! always surfaces the §20.7 warning.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::error::{PluginError, PluginResult};
use crate::plugin_type::PluginType;
use crate::version::{check_abi_version, check_min_app_version};

/// Filename the manager looks for next to a plugin's shared library.
pub const MANIFEST_FILENAME: &str = "plugin.json";

/// One of the four RFC-5545 frequencies a calendar adapter can
/// claim support for. Mirrors the frontend's `Freq` (minus `NONE`,
/// which isn't a recurrence at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceFreq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

fn all_frequencies() -> Vec<RecurrenceFreq> {
    use RecurrenceFreq::*;
    vec![Daily, Weekly, Monthly, Yearly]
}

fn yes() -> bool {
    true
}

/// Which recurrence shapes a calendar adapter can faithfully
/// round-trip. Declared (optionally) in `plugin.json` so the
/// EventDialog can grey out options the source can't store — e.g.
/// EWS has no yearly interval, so it omits `yearly` from
/// `interval_frequencies`.
///
/// **Permissive by default**: every field defaults to "fully
/// supported", and the whole struct defaults to "everything" when
/// the manifest omits the `recurrence` block entirely. A plugin
/// author therefore only spells out what they *restrict* — a one-
/// line override like `{"interval_frequencies": ["daily","weekly",
/// "monthly"]}` keeps all the other axes at full support. That
/// keeps the common case (full RFC-5545) zero-config and existing
/// manifests working unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceCapabilities {
    /// Frequencies offered in the "Repeat" dropdown.
    #[serde(default = "all_frequencies")]
    pub frequencies: Vec<RecurrenceFreq>,
    /// Frequencies whose INTERVAL (>1) the source can store. EWS
    /// drops `yearly` here — its AbsoluteYearly/RelativeYearly
    /// patterns carry no Interval element.
    #[serde(default = "all_frequencies")]
    pub interval_frequencies: Vec<RecurrenceFreq>,
    /// Relative monthly ("third Wednesday") — BYDAY=Nxx on MONTHLY.
    #[serde(default = "yes")]
    pub relative_monthly: bool,
    /// Relative yearly ("first Friday of March").
    #[serde(default = "yes")]
    pub relative_yearly: bool,
    /// Weekly weekday picker (BYDAY on WEEKLY).
    #[serde(default = "yes")]
    pub weekly_byday: bool,
    /// COUNT end mode ("after N occurrences").
    #[serde(default = "yes")]
    pub count: bool,
    /// UNTIL end mode ("until a date").
    #[serde(default = "yes")]
    pub until: bool,
}

impl Default for RecurrenceCapabilities {
    fn default() -> Self {
        Self {
            frequencies: all_frequencies(),
            interval_frequencies: all_frequencies(),
            relative_monthly: true,
            relative_yearly: true,
            weekly_byday: true,
            count: true,
            until: true,
        }
    }
}

/// Parsed `plugin.json`. All fields are owned strings so the
/// manifest survives the file handle being dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Stable reverse-DNS identifier, e.g.
    /// `"com.aperio.cal-adapter-local"`. Used as the primary key
    /// in the loaded-plugins map; two different shared libraries
    /// can't claim the same id.
    pub id: String,

    /// Human-readable display name. The plugin already localises
    /// it if it cares — host doesn't try to i18n this.
    pub name: String,

    /// SemVer string. Validated via [`crate::Version::parse`] at
    /// load time so the `compare` UI surface can show ordered
    /// numbers in the §20.9 "Plugin aktualisieren" dialog.
    pub version: String,

    /// Plugin-type tag. See [`PluginType`] for the canonical set.
    pub plugin_type: PluginType,

    /// Feature surface for `calendar-adapter` plugins — any subset
    /// of `["calendar", "tasks", "contacts"]`. Other plugin types
    /// MAY leave it absent.
    #[serde(default)]
    pub capabilities: Vec<Capability>,

    /// ABI version emitted by the plugin. MUST equal the host's
    /// [`crate::ABI_VERSION`]; checked at load time.
    pub abi_version: u32,

    /// Minimum Aperio version that can run this plugin. Compared
    /// against the host's `CARGO_PKG_VERSION` via
    /// [`crate::check_min_app_version`].
    pub min_app_version: String,

    /// Optional author label for the Settings → Plugins panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Optional one-line description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Signed flag — preserved for forward-compat. See module
    /// docs for the "no signing in this phase" policy.
    #[serde(default)]
    pub signed: bool,

    /// Which recurrence shapes this adapter can store, surfaced to
    /// the EventDialog so it can grey out unsupported options.
    /// Absent → [`RecurrenceCapabilities::default`] (full RFC-5545),
    /// so existing manifests and non-calendar plugins need no change.
    #[serde(default)]
    pub recurrence: RecurrenceCapabilities,
}

impl PluginManifest {
    /// Parse a `plugin.json` from disk. Any IO + JSON shape
    /// problems are surfaced through [`PluginError`].
    pub fn read_from(path: impl AsRef<Path>) -> PluginResult<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        Self::from_bytes(&bytes)
    }

    /// Parse a `plugin.json` from in-memory bytes. Used by the
    /// future `.aperio` archive extractor — it pulls the manifest
    /// out of the ZIP before writing anything to disk.
    pub fn from_bytes(bytes: &[u8]) -> PluginResult<Self> {
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate_basic()?;
        Ok(manifest)
    }

    /// Cheap sanity checks that don't require any host state:
    /// non-empty id + name + version, parseable semver. Heavier
    /// gates (ABI version match, min-app-version against the
    /// running build) live in [`Self::compatible_with`] because
    /// they need the host's own version numbers as inputs.
    fn validate_basic(&self) -> PluginResult<()> {
        if self.id.trim().is_empty() {
            return Err(PluginError::Manifest("id must not be empty".into()));
        }
        if self.name.trim().is_empty() {
            return Err(PluginError::Manifest("name must not be empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(PluginError::Manifest("version must not be empty".into()));
        }
        if self.min_app_version.trim().is_empty() {
            return Err(PluginError::Manifest(
                "min_app_version must not be empty".into(),
            ));
        }
        // Round-trip both versions through the parser so we fail
        // fast on author typos rather than panicking deep inside a
        // compare on first use.
        crate::Version::parse(&self.version)?;
        crate::Version::parse(&self.min_app_version)?;
        Ok(())
    }

    /// Run the host-side compatibility gates: ABI match + min app
    /// version. Returns `Ok(())` when the manifest can be loaded
    /// against the supplied `app_version` (typically
    /// `env!("CARGO_PKG_VERSION")` at the call site).
    pub fn compatible_with(&self, app_version: &str) -> PluginResult<()> {
        check_abi_version(self.abi_version)?;
        check_min_app_version(&self.min_app_version, app_version)?;
        Ok(())
    }

    /// True iff `cap` appears in the manifest's `capabilities`
    /// array. The future `as_calendar_feature(plugin_id)`
    /// resolver uses this to skip plugins that don't actually
    /// declare the right surface.
    pub fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::ABI_VERSION;

    fn sample_manifest_json() -> String {
        format!(
            r#"{{
                "id": "com.aperio.cal-adapter-local",
                "name": "Aperio Local",
                "version": "0.1.0",
                "plugin_type": "calendar-adapter",
                "capabilities": ["calendar", "tasks", "contacts"],
                "abi_version": {ABI_VERSION},
                "min_app_version": "0.1.0",
                "author": "Aperio Contributors",
                "description": "Bundled SQLite-backed local adapter."
            }}"#
        )
    }

    #[test]
    fn parses_full_manifest() {
        let m = PluginManifest::from_bytes(sample_manifest_json().as_bytes()).expect("parses");
        assert_eq!(m.id, "com.aperio.cal-adapter-local");
        assert_eq!(m.name, "Aperio Local");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.plugin_type, PluginType::CalendarAdapter);
        assert_eq!(
            m.capabilities,
            vec![
                Capability::Calendar,
                Capability::Tasks,
                Capability::Contacts
            ]
        );
        assert_eq!(m.abi_version, ABI_VERSION);
        assert_eq!(m.author.as_deref(), Some("Aperio Contributors"));
        assert!(!m.signed);
    }

    #[test]
    fn parses_minimal_manifest_with_only_required_fields() {
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "notification",
                "abi_version": {ABI_VERSION},
                "min_app_version": "1.0.0"
            }}"#
        );
        let m = PluginManifest::from_bytes(json.as_bytes()).expect("parses");
        assert!(m.capabilities.is_empty());
        assert!(m.author.is_none());
        assert!(m.description.is_none());
        assert!(!m.signed);
    }

    #[test]
    fn rejects_empty_id() {
        let json = format!(
            r#"{{
                "id": "",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "notification",
                "abi_version": {ABI_VERSION},
                "min_app_version": "1.0.0"
            }}"#
        );
        let err = PluginManifest::from_bytes(json.as_bytes()).unwrap_err();
        match err {
            PluginError::Manifest(msg) => assert!(msg.contains("id")),
            other => panic!("expected Manifest, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_version_string() {
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "not-a-version",
                "plugin_type": "notification",
                "abi_version": {ABI_VERSION},
                "min_app_version": "1.0.0"
            }}"#
        );
        let err = PluginManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, PluginError::Semver { .. }));
    }

    #[test]
    fn unknown_plugin_type_round_trips() {
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "future-type",
                "abi_version": {ABI_VERSION},
                "min_app_version": "1.0.0"
            }}"#
        );
        let m = PluginManifest::from_bytes(json.as_bytes()).expect("parses");
        assert_eq!(
            m.plugin_type,
            PluginType::Unknown("future-type".to_string())
        );
        assert!(!m.plugin_type.is_known());
    }

    #[test]
    fn compatible_with_passes_for_current_build() {
        let m = PluginManifest::from_bytes(sample_manifest_json().as_bytes()).expect("parses");
        // Sample asks for 0.1.0; pretend the host is the same.
        m.compatible_with("0.1.0").expect("compatible");
    }

    #[test]
    fn compatible_with_rejects_old_running_app() {
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "notification",
                "abi_version": {ABI_VERSION},
                "min_app_version": "2.0.0"
            }}"#
        );
        let m = PluginManifest::from_bytes(json.as_bytes()).expect("parses");
        let err = m.compatible_with("0.1.0").unwrap_err();
        assert!(matches!(err, PluginError::AppTooOld { .. }));
    }

    #[test]
    fn compatible_with_rejects_abi_mismatch() {
        let bad_abi = ABI_VERSION + 1;
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "notification",
                "abi_version": {bad_abi},
                "min_app_version": "0.1.0"
            }}"#
        );
        let m = PluginManifest::from_bytes(json.as_bytes()).expect("parses");
        let err = m.compatible_with("0.1.0").unwrap_err();
        assert!(matches!(err, PluginError::AbiMismatch { .. }));
    }

    #[test]
    fn has_capability_returns_membership() {
        let m = PluginManifest::from_bytes(sample_manifest_json().as_bytes()).expect("parses");
        assert!(m.has_capability(&Capability::Calendar));
        assert!(m.has_capability(&Capability::Tasks));
        assert!(m.has_capability(&Capability::Contacts));
        assert!(!m.has_capability(&Capability::Unknown("nope".into())));
    }

    #[test]
    fn recurrence_absent_defaults_to_full_support() {
        // The sample manifest has no `recurrence` block — every axis
        // must come back fully supported.
        let m = PluginManifest::from_bytes(sample_manifest_json().as_bytes()).expect("parses");
        let r = &m.recurrence;
        assert_eq!(r.frequencies.len(), 4);
        assert_eq!(r.interval_frequencies.len(), 4);
        assert!(r.relative_monthly);
        assert!(r.relative_yearly);
        assert!(r.weekly_byday);
        assert!(r.count);
        assert!(r.until);
    }

    #[test]
    fn recurrence_partial_override_keeps_other_axes_full() {
        // EWS-style declaration: only `interval_frequencies` is
        // restricted (no yearly); everything else must stay full.
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "calendar-adapter",
                "abi_version": {ABI_VERSION},
                "min_app_version": "1.0.0",
                "recurrence": {{
                    "interval_frequencies": ["daily", "weekly", "monthly"]
                }}
            }}"#
        );
        let m = PluginManifest::from_bytes(json.as_bytes()).expect("parses");
        let r = &m.recurrence;
        // The restricted axis took the override.
        assert_eq!(
            r.interval_frequencies,
            vec![
                RecurrenceFreq::Daily,
                RecurrenceFreq::Weekly,
                RecurrenceFreq::Monthly,
            ],
        );
        // Every other axis stayed at the permissive default.
        assert_eq!(r.frequencies.len(), 4);
        assert!(r.relative_monthly);
        assert!(r.relative_yearly);
        assert!(r.weekly_byday);
        assert!(r.count);
        assert!(r.until);
    }

    #[test]
    fn manifest_round_trips_through_serde() {
        let original =
            PluginManifest::from_bytes(sample_manifest_json().as_bytes()).expect("parses");
        let json = serde_json::to_string(&original).expect("serialise");
        let back = PluginManifest::from_bytes(json.as_bytes()).expect("re-parses");
        assert_eq!(original, back);
    }
}
