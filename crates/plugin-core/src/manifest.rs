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

use crate::account_schema::AccountSchema;
use crate::capability::Capability;
use crate::error::{PluginError, PluginResult};
use crate::plugin_type::PluginType;
use crate::strings::StringCatalogue;
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
    /// An explicit monthly day-of-month can be stored. Vikunja's monthly
    /// recurrence repeats on the task's due-date day implicitly and can't
    /// take a separate day number, so it sets this `false`; the task UI
    /// then disables the "day of month" field. Calendar events always
    /// store BYMONTHDAY, so the default stays `true`.
    #[serde(default = "yes")]
    pub monthly_day_of_month: bool,
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
            monthly_day_of_month: true,
            count: true,
            until: true,
        }
    }
}

/// Which task-organisation features a `tasks`-capable adapter can
/// faithfully round-trip. Declared (optionally) in `plugin.json` so
/// the task UI can show / grey-out the right affordances — e.g.
/// Vikunja and Todoist nest their projects, Microsoft To Do and
/// Google Tasks keep flat lists; Todoist groups tasks into sections,
/// the others don't.
///
/// Defaults track what cal-core models *natively* rather than the
/// richest backend: a backend that omits the block (or a field) is
/// assumed flat-but-with-subtasks — the shape the local store and
/// most simple adapters have. Each backend then widens (Vikunja:
/// `nested_projects` + `sections`) or narrows (a step-only backend:
/// `subtasks` with `max_subtask_depth = 1`).
/// How a task adapter adds members to a list (DESIGN §9.7): by searching
/// a user directory (Vikunja) or by inviting a raw email address
/// (Todoist). Drives which control the members dialog renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemberAddMethod {
    /// Search the instance/workspace directory and pick a user.
    #[default]
    Search,
    /// Type a raw email address to invite (pending until accepted).
    Email,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCapabilities {
    /// Task lists (projects) nest into a tree. Flat backends leave
    /// this `false`; the UI then renders a depth-0 forest.
    #[serde(default)]
    pub nested_projects: bool,
    /// Tasks can carry subtasks (`Task.parent_id`). cal-core models
    /// this natively, so it defaults to `true`.
    #[serde(default = "yes")]
    pub subtasks: bool,
    /// Maximum subtask nesting depth. `None` = unlimited. Backends
    /// that only do one level (Google Tasks, Microsoft "steps") set
    /// `Some(1)`.
    #[serde(default)]
    pub max_subtask_depth: Option<u32>,
    /// Tasks can be grouped into sections within a container
    /// (Todoist sections, Vikunja buckets).
    #[serde(default)]
    pub sections: bool,
    /// The adapter can create / rename / delete sections at the source
    /// (Todoist sections, Vikunja kanban buckets). Defaults `false`; the
    /// UI gates the section create/rename/delete controls on it. Coloring
    /// a section is independent — it's always a local override, so it's
    /// offered wherever `sections` is true.
    #[serde(default)]
    pub manageable_sections: bool,
    /// More than one label per task, beyond cal-core's single
    /// `color_label` slot.
    #[serde(default)]
    pub multiple_labels: bool,
    /// Tasks support a recurrence rule.
    #[serde(default = "yes")]
    pub task_recurrence: bool,
    /// The source can store the "in progress" status as a distinct state.
    /// Backends with only open/done (Google Tasks, Vikunja, Todoist) set
    /// this `false`: an `in_progress` write collapses to `open` on
    /// read-back, so the UI skips the auto-schedule-to-today a "started"
    /// task would otherwise trigger (a status that can't stick shouldn't
    /// move the date). Defaults `true` — cal-core-native: local, CalDAV,
    /// Exchange and Microsoft To Do all keep it.
    #[serde(default = "yes")]
    pub supports_in_progress: bool,
    /// A task can be moved to a different container. Todoist defers
    /// cross-project moves, so it sets this `false`.
    #[serde(default = "yes")]
    pub move_between_projects: bool,
    /// The adapter can create new task lists (projects) at the source.
    /// Defaults `false` — an adapter opts in once it implements
    /// `TasksFeature::create_task_list`; the UI gates its "new list in
    /// this account" affordance on it.
    #[serde(default)]
    pub create_lists: bool,
    /// The adapter can delete task lists at the source. Same opt-in
    /// shape as `create_lists`.
    #[serde(default)]
    pub delete_lists: bool,
    /// The adapter can manage a list's membership/sharing (the sidebar's
    /// "manage members" entry + the members dialog). Vikunja + Todoist
    /// opt in; flat/personal backends (local, Google Tasks, MS To Do)
    /// leave it `false` so the UI doesn't offer a control they can't
    /// fulfil.
    #[serde(default)]
    pub manageable: bool,
    /// How members are added when `manageable`: by user-directory search
    /// (Vikunja) or by raw-email invite (Todoist). Drives the members
    /// dialog's add control; ignored when `manageable` is `false`.
    #[serde(default)]
    pub member_add_by: MemberAddMethod,
    /// Which recurrence shapes this adapter can store for tasks (mirrors
    /// the calendar side's per-account recurrence caps). Absent → full
    /// support, so the task recurrence editor offers everything. A backend
    /// with a simpler model narrows it — Vikunja stores a plain interval
    /// (`repeat_after` seconds) plus a monthly mode, so it drops yearly,
    /// the weekday picker, an explicit day-of-month and the COUNT / UNTIL
    /// end modes; the editor greys those out instead of dropping them
    /// silently on save.
    #[serde(default)]
    pub recurrence: RecurrenceCapabilities,
}

impl Default for TaskCapabilities {
    fn default() -> Self {
        Self {
            nested_projects: false,
            subtasks: true,
            max_subtask_depth: None,
            sections: false,
            manageable_sections: false,
            multiple_labels: false,
            task_recurrence: true,
            supports_in_progress: true,
            move_between_projects: true,
            create_lists: false,
            delete_lists: false,
            manageable: false,
            member_add_by: MemberAddMethod::Search,
            recurrence: RecurrenceCapabilities::default(),
        }
    }
}

/// One account-bearing adapter a loaded plugin serves.
///
/// What a connect picker needs and nothing more. It is assembled from the
/// manifests at call time rather than kept anywhere, so enabling or disabling a
/// plugin changes the answer immediately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterKindInfo {
    /// The value accounts of this adapter carry in `accounts.adapter_kind`.
    pub kind: String,
    /// The plugin's display name — the label to use when the app has no
    /// translation for this kind, which is the normal case for a third-party
    /// plugin.
    pub name: String,
    pub plugin_id: String,
    /// Whether accounts of this adapter own calendars and task lists.
    pub owns_containers: bool,
    /// Whether it can be connected through the generic schema-driven flow.
    /// False for the adapters still on the host's older per-kind path.
    pub declares_account_schema: bool,
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

    /// Which task-organisation features this adapter supports
    /// (nested projects, sections, subtasks, …), surfaced to the
    /// task UI. Absent → [`TaskCapabilities::default`] (flat lists
    /// with subtasks), so existing manifests and non-task plugins
    /// need no change.
    #[serde(default)]
    pub tasks: TaskCapabilities,

    /// What this plugin needs in order to have an account — the fields to ask
    /// for, which of them are secrets, and whether it signs in via OAuth. See
    /// [`AccountSchema`]. Absent means the host has no generic way to connect
    /// this plugin, which is the correct answer for a notification channel and
    /// for the adapters still on the host's older per-kind connect path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountSchema>,

    /// The value this plugin's accounts carry in `accounts.adapter_kind` — the
    /// short, stable routing key that identifies which adapter a row belongs
    /// to (`"caldav"`, `"webex"`, …).
    ///
    /// Declared here rather than enumerated in the host: this is the mapping
    /// that used to force an edit to the core before any adapter could exist.
    /// The host builds the reverse map by asking the loaded plugins.
    ///
    /// It is deliberately NOT the plugin id. The kind is persisted in every
    /// account row and travels in every sync payload, so it has to stay
    /// byte-stable for the life of the data; the plugin id is free to change
    /// when a plugin is renamed or re-homed. A new adapter is free to use its
    /// id as its kind — nothing stops it — but the two are separate promises.
    ///
    /// Absent for plugins whose accounts the host does not route this way
    /// (sync adapters live in `user_prefs`, notification channels have no
    /// accounts at all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_kind: Option<String>,

    /// This plugin's own text, in the languages it speaks — see
    /// [`crate::strings`] for why it lives here rather than in the host's
    /// locale files, and for the resolution order.
    ///
    /// Keys are referenced from wherever the plugin declares a label: the
    /// `label_key` / `hint_key` of an [`AccountSchema`] field, and the
    /// `label_key` of a join detail on a meeting. Absent is fine and common —
    /// a plugin that writes only verbatim `label`s renders in whatever
    /// language it wrote them.
    ///
    /// This is a DECLARATION as much as a catalogue: the host reads the
    /// language list from it to build the invitation-language picker, and it
    /// has to be able to do that without loading the shared library.
    #[serde(default, skip_serializing_if = "StringCatalogue::is_empty")]
    pub strings: StringCatalogue,
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
        // A malformed account schema fails HERE, while the plugin is loading,
        // rather than halfway through creating an account with secrets already
        // written to the keychain.
        if let Some(account) = &self.account {
            account.validate()?;
        }
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
    fn tasks_absent_defaults_to_flat_with_subtasks() {
        let m = PluginManifest::from_bytes(sample_manifest_json().as_bytes()).expect("parses");
        let tk = &m.tasks;
        assert!(!tk.nested_projects);
        assert!(tk.subtasks);
        assert_eq!(tk.max_subtask_depth, None);
        assert!(!tk.sections);
        assert!(!tk.multiple_labels);
        assert!(tk.task_recurrence);
        assert!(tk.move_between_projects);
    }

    #[test]
    fn tasks_partial_override_keeps_other_axes_default() {
        // Vikunja-style declaration: nested projects + sections on,
        // everything else stays at the cal-core-native default.
        let json = format!(
            r#"{{
                "id": "x.y",
                "name": "X",
                "version": "1.0.0",
                "plugin_type": "calendar-adapter",
                "abi_version": {ABI_VERSION},
                "min_app_version": "1.0.0",
                "tasks": {{
                    "nested_projects": true,
                    "sections": true,
                    "move_between_projects": false
                }}
            }}"#
        );
        let m = PluginManifest::from_bytes(json.as_bytes()).expect("parses");
        let tk = &m.tasks;
        assert!(tk.nested_projects);
        assert!(tk.sections);
        assert!(!tk.move_between_projects);
        // Untouched axes keep their defaults.
        assert!(tk.subtasks);
        assert!(tk.task_recurrence);
        assert!(!tk.multiple_labels);
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
