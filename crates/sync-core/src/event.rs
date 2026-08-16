//! `SyncEvent` — the wire-format enum for every mutation that flows
//! through Aperio's append-only event log.
//!
//! ## Design highlights
//!
//! - **One variant per spec-listed event type** from DESIGN.md §19.2.
//!   The serde tag is the canonical `event.created` / `task.updated`
//!   string the spec dictates, so a log file written by Aperio 1.x
//!   stays readable by future versions even if they add variants.
//! - **Untyped payloads are deliberate.** A `task.updated` event
//!   carries a partial diff as `serde_json::Value` rather than a
//!   strongly-typed `TaskFieldsDiff`. Reason: the source-of-truth
//!   field set lives in `cal-core::Task` which has its own evolution
//!   trajectory (Phase 10l added `addresses` for example). Keeping
//!   the diff loose at the sync layer means we don't have to bump
//!   `schema_version` every time a `cal-core` struct grows a field —
//!   forward-compatible by construction.
//! - **EventId is a string, not a generic monotonic counter.** The
//!   convention is `evt_` + a 26-char ULID; new generators get those
//!   for free from the local mint. The encoding is opaque on the
//!   wire so future devices can substitute any unique string.
//! - **Field-level merging is enabled by the diff shape.** An
//!   `event.updated` payload only carries fields the user touched.
//!   Two devices that touch different fields on the same row merge
//!   cleanly without prompting; only when the same field appears in
//!   two not-yet-applied events from different devices do we surface
//!   a conflict (§19.3).
//!
//! ## What this module does NOT do
//!
//! It defines the data shape only. Writing events into a log file
//! is the writer's job (Phase Sb); applying them to the local SQLite
//! cache is the applier's job (Phase Sc); generating them from user
//! mutations is wired in at the command layer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::device::DeviceId;

/// Opaque identifier for one sync event.
///
/// Generators pick a string that's globally unique across all
/// devices — current convention is `evt_` + a 26-char Crockford
/// base32 ULID (lexicographically sortable, includes a timestamp).
/// The applier uses this for idempotency: a log file that's read
/// twice produces the same `EventId`s, and the applier skips ones
/// it has already integrated.
pub type EventId = String;

/// One entry in the append-only log.
///
/// On disk this is a single line of JSON (the `sync/log/*.jsonl`
/// format). The envelope carries the metadata every event needs —
/// id, originator, timestamp — and `kind` carries the event-type-
/// specific payload.
///
/// We split the envelope from `SyncEvent` so the metadata fields
/// don't have to be repeated in every variant. The wire shape after
/// serialisation:
///
/// ```json
/// {
///   "id": "evt_01jf3k...",
///   "device_id": "8d2c...",
///   "timestamp": "2025-05-12T09:14:22.341Z",
///   "type": "event.updated",
///   "payload": { ... }
/// }
/// ```
///
/// `type` and `payload` come from the `#[serde(tag, content)]` on
/// `SyncEvent`; the envelope contributes the rest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEnvelope {
    /// Globally unique id — see [`EventId`]'s docs.
    pub id: EventId,
    /// The originator's [`DeviceId`]. Conflict-resolution needs to
    /// know "did this event come from me or from another device?"
    /// when computing field-level merges.
    pub device_id: DeviceId,
    /// RFC 3339 timestamp at the moment the event was minted.
    /// Used to chronologically order events from multiple devices.
    /// Ties broken by `id` lexicographic order (ULIDs encode the
    /// timestamp + a random tail, so they break ties cheaply).
    pub timestamp: DateTime<Utc>,
    /// The event itself — `#[serde(flatten)]` puts `type` and
    /// `payload` at the top level alongside `id` / `device_id` /
    /// `timestamp` per the §19.2 wire schema.
    #[serde(flatten)]
    pub event: SyncEvent,
}

impl EventEnvelope {
    /// Convenience constructor used by writers that want the
    /// timestamp set to "right now" and the id minted from a fresh
    /// ULID.
    pub fn new(device_id: DeviceId, event: SyncEvent) -> Self {
        Self {
            id: mint_event_id(),
            device_id,
            timestamp: Utc::now(),
            event,
        }
    }
}

/// Mint a new opaque event id. The current implementation is a
/// timestamp-prefixed pseudo-random suffix that's sortable on
/// disk; consumers must treat it as opaque so we can swap in a
/// proper ULID library later without a schema bump.
fn mint_event_id() -> EventId {
    // `chrono` gives us a monotonic enough source for the prefix.
    // The randomness comes from the OS via `uuid::Uuid::new_v4`;
    // we take 10 hex chars off the end as a tie-breaker. Total
    // length is around 30 chars which fits the spec's example
    // shape.
    let now = Utc::now().timestamp_millis();
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    let tail = &uuid[uuid.len().saturating_sub(10)..];
    format!("evt_{now:013x}{tail}")
}

/// Every mutation that propagates across devices.
///
/// Variants are listed in the same order as DESIGN.md §19.2 so a
/// reader can compare 1:1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload")]
#[serde(rename_all = "snake_case")]
pub enum SyncEvent {
    /// Local-adapter event was created. Payload is the full event
    /// row — applier inserts it verbatim. External-adapter events
    /// (Google / iCloud / Graph / EWS) do NOT generate sync events;
    /// those providers do their own sync (§19.12).
    #[serde(rename = "event.created")]
    EventCreated(EventPayload),

    /// Local event was edited. Payload carries only the fields the
    /// user touched — the applier merges them into the existing row
    /// (or, on the conflict path, records the diff for resolution).
    #[serde(rename = "event.updated")]
    EventUpdated(PartialPayload),

    /// Local event was deleted.
    #[serde(rename = "event.deleted")]
    EventDeleted(IdPayload),

    /// Which events Aperio has been told mean the same appointment
    /// (DESIGN-event-groups.md). Carries the WHOLE membership, never a
    /// diff: a group is small, only meaningful entire, and two devices
    /// grouping the same events independently must converge rather than
    /// interleave additions and removals into a set neither of them
    /// asked for. Last writer wins, on a value a user can see and redo.
    ///
    /// Synced, unlike the meeting binding beside it, because the
    /// grouping exists nowhere else: a device that has not been told
    /// stays convinced it is looking at separate appointments.
    #[serde(rename = "event_group.updated")]
    EventGroupUpdated(EventPayload),

    /// The user said two events are NOT the same appointment, so the offer to
    /// group them stops being made (DESIGN-event-groups.md, Stufe 3).
    ///
    /// Synced, and the easiest thing in this file to reason about: the
    /// declines are a set that only ever GROWS, so two devices declining
    /// different pairs converge by union and no ordering rule is needed. That
    /// is a deliberate contrast with the group events above, whose membership
    /// moves in both directions.
    #[serde(rename = "event_group_suggestion.declined")]
    EventGroupSuggestionDeclined(EventPayload),

    /// A group was dissolved. The events themselves are untouched —
    /// this says only that Aperio no longer claims they are one.
    #[serde(rename = "event_group.dissolved")]
    EventGroupDissolved(IdPayload),

    /// Local task created.
    #[serde(rename = "task.created")]
    TaskCreated(EventPayload),

    /// Local task edited — same partial-diff shape as `event.updated`.
    #[serde(rename = "task.updated")]
    TaskUpdated(PartialPayload),

    /// Local task deleted.
    #[serde(rename = "task.deleted")]
    TaskDeleted(IdPayload),

    /// Task list created (local adapter). External-adapter task
    /// lists sync via their own provider APIs and don't generate
    /// sync events here.
    #[serde(rename = "task_list.created")]
    TaskListCreated(EventPayload),

    /// Task list metadata changed (rename, recolor, embedded flag).
    #[serde(rename = "task_list.updated")]
    TaskListUpdated(PartialPayload),

    /// Task list deleted.
    #[serde(rename = "task_list.deleted")]
    TaskListDeleted(IdPayload),

    /// Section (Vikunja bucket / Todoist section) created in a local
    /// list. Payload is the full `Section` row.
    #[serde(rename = "section.created")]
    SectionCreated(EventPayload),

    /// Section renamed / reordered. Sections are simple metadata, so
    /// the applier takes the full row last-write-wins rather than
    /// running the field-level conflict merge the richer rows use.
    #[serde(rename = "section.updated")]
    SectionUpdated(PartialPayload),

    /// Section deleted — its tasks fall back to ungrouped.
    #[serde(rename = "section.deleted")]
    SectionDeleted(IdPayload),

    /// Local calendar added.
    #[serde(rename = "calendar.created")]
    CalendarCreated(EventPayload),

    /// Local calendar settings changed (rename, color, default
    /// reminders).
    #[serde(rename = "calendar.updated")]
    CalendarUpdated(PartialPayload),

    /// Local calendar removed.
    #[serde(rename = "calendar.deleted")]
    CalendarDeleted(IdPayload),

    /// Color label added (DESIGN.md §8).
    #[serde(rename = "color_label.created")]
    ColorLabelCreated(EventPayload),

    /// Color label edited.
    #[serde(rename = "color_label.updated")]
    ColorLabelUpdated(PartialPayload),

    /// Color label deleted.
    #[serde(rename = "color_label.deleted")]
    ColorLabelDeleted(IdPayload),

    /// A day-marker vocabulary entry was added or edited.
    ///
    /// One event for both: the vocabulary is small and a marker is written
    /// whole, so a create and an edit are the same upsert on the receiving
    /// side. Splitting them would buy a distinction nothing reads.
    #[serde(rename = "day_marker.written")]
    DayMarkerWritten(EventPayload),

    /// A day-marker vocabulary entry was removed. The day rows keep the id —
    /// readers resolve against the vocabulary and drop what is gone.
    #[serde(rename = "day_marker.deleted")]
    DayMarkerDeleted(IdPayload),

    /// One day's log was set. Carries the whole row, keyed by the day.
    ///
    /// Last-write-wins on the day, like the rest of the store: two devices
    /// editing the SAME day between two rounds keep the later edit, not the
    /// union. Deliberate — a union would make REMOVING a marker impossible to
    /// propagate, which is the worse failure. An emptied day arrives as a log
    /// with nothing on it, and the applier deletes the row.
    #[serde(rename = "day_log.set")]
    DayLogSet(EventPayload),

    /// Community plugin was installed locally — sync the metadata
    /// (not the binary) so other devices can offer the user the
    /// matching install.
    #[serde(rename = "plugin.installed")]
    PluginInstalled(PluginPayload),

    /// Plugin metadata changed (version bump after auto-update on
    /// one device).
    #[serde(rename = "plugin.updated")]
    PluginUpdated(PluginPayload),

    /// Plugin uninstalled — other devices clear their "plugin
    /// missing" markers for this id.
    #[serde(rename = "plugin.uninstalled")]
    PluginUninstalled(IdPayload),

    /// External account (CalDAV, iCal, EWS, Vikunja, Todoist,
    /// Google, Microsoft Graph) was added locally. The payload
    /// carries only non-secret metadata — `display_name`,
    /// `adapter_kind`, `config_json` (server URL, username,
    /// etc.). Secrets (passwords, OAuth tokens) stay in the
    /// device's keychain; other devices surface the
    /// "credentials missing" wizard so the user enters them
    /// per-device. The applier upserts the row; on the device
    /// that originated the event it's a no-op (the row already
    /// exists from the create command's repo write).
    #[serde(rename = "account.created")]
    AccountCreated(AccountPayload),

    /// External account metadata changed — typically a rename
    /// (`display_name`) or a config tweak. Same payload shape
    /// as `account.created`; the applier upserts so a missing
    /// row from a device that hadn't seen the create event
    /// still ends up in the right state.
    #[serde(rename = "account.updated")]
    AccountUpdated(AccountPayload),

    /// External account removed. Other devices delete the row +
    /// any "credentials missing" badge that may have been
    /// hanging on it.
    #[serde(rename = "account.deleted")]
    AccountDeleted(IdPayload),

    /// A secret for an external account (CalDAV/WebDAV password, OAuth
    /// refresh token, API token) was set or changed.
    ///
    /// **Only ever emitted while end-to-end encryption is enabled.** The
    /// payload carries the plaintext secret, so this variant must only
    /// exist inside an encrypted log blob — the emit site asserts E2E is
    /// on before appending, and the E2E-disable downgrade strips these
    /// events so they never reach a plaintext log/snapshot. The applier
    /// writes the secret into the receiving device's keychain so the
    /// account works without re-entering credentials (§19.2.3).
    #[serde(rename = "credential.set")]
    CredentialSet(CredentialPayload),

    /// A secret slot for an external account was cleared (E2E only —
    /// same gating as `credential.set`). The applier removes that slot
    /// from the receiving device's keychain.
    #[serde(rename = "credential.cleared")]
    CredentialCleared(CredentialSlotPayload),

    /// Keyboard shortcut for an action was set or rebound
    /// (§15.10 + §19.2.1).
    #[serde(rename = "shortcut.set")]
    ShortcutSet(ShortcutPayload),

    /// Shortcut for an action restored to the default binding.
    #[serde(rename = "shortcut.reset")]
    ShortcutReset(ShortcutKeyPayload),

    /// Shortcut for an action deliberately cleared with no
    /// replacement (the user wants this action *not* to have a
    /// keyboard binding at all).
    #[serde(rename = "shortcut.cleared")]
    ShortcutCleared(ShortcutKeyPayload),

    /// Synchronisable app setting changed. The whitelist of keys
    /// that may emit this event lives in §19.2.1 — anything not on
    /// the list stays in local-only user_prefs.
    #[serde(rename = "settings.updated")]
    SettingsUpdated(SettingsPayload),
}

/// Payload for `*.created` events — carries the full row as
/// `serde_json::Value`. We keep it untyped at this layer so the
/// log schema doesn't have to be bumped every time a `cal-core`
/// struct grows a field. The applier uses serde to deserialise
/// into the concrete struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventPayload {
    /// Primary key — the same id the local row has in SQLite.
    /// Required so the applier can look up the target row and
    /// insert it (or skip if it already exists).
    pub id: String,
    /// The full row data, untyped. Concrete deserialisation
    /// happens in the applier where it has the target struct
    /// available.
    pub fields: serde_json::Value,
}

/// Payload for `*.updated` events. Same shape as `EventPayload`
/// but `fields` carries ONLY the touched columns — the applier
/// merges them into the existing row. Untouched columns survive
/// the merge intact, which is the foundation of the field-level
/// auto-merge in §19.3.
pub type PartialPayload = EventPayload;

/// Payload for `*.deleted` events — only the id. We carry the id
/// in its own struct (rather than reusing `EventPayload` with an
/// empty `fields`) so the wire shape stays compact for the most
/// common event type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdPayload {
    pub id: String,
}

/// Payload for `plugin.installed` / `plugin.updated`. The binary
/// itself is NOT shipped through sync; only the metadata, so
/// other devices know to offer the matching install (§20.8).
///
/// The `name` and `plugin_type` fields are optional for
/// backward compatibility — older Aperio devices emit payloads
/// without them, and serde's `default` keeps the deserialise
/// path tolerant. Newer devices fill them so the §20.8 "Plugin
/// benötigt" dialog can show the user a recognisable name +
/// type rather than just an opaque reverse-DNS id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginPayload {
    /// Plugin identifier (matches the manifest's `id` field).
    pub id: String,
    /// Semver of the installed version. Used both for "what version
    /// should I install?" and for the `plugin.updated` propagation.
    pub version: String,
    /// Optional source — distribution URL, registry name, …
    /// Free-form for now; later the registry contract will pin a
    /// schema (Phase 20.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Human-readable plugin name from the manifest. Optional
    /// for backward compat with older Aperio devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Plugin-type wire string (`calendar-adapter`,
    /// `sync-adapter`, `videoconference-adapter`,
    /// `notification`). Optional for backward compat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_type: Option<String>,
}

/// Payload for `account.created` / `account.updated`. Non-secret
/// account metadata sufficient to recreate the row on another
/// device (`adapter_kind`, `display_name`, `config_json`). The
/// secret half (CalDAV password, OAuth refresh token, …) stays
/// in the originating device's keychain — receiving devices
/// surface the "credentials missing" wizard so the user enters
/// the secret per-device. `created_at` / `updated_at` round-trip
/// to keep the snapshot path and the event path producing the
/// same timestamps on the receiving side.
/// Adapter kinds that describe something belonging to ONE machine, and must
/// therefore never cross the wire in either direction.
///
/// `local` is the built-in store every device has its own copy of; a row for it
/// arriving from a peer would overwrite that device's bootstrap timestamps.
/// `device_calendar` is the phone's own calendar and reminder store, reached
/// through an OS permission grant — an account for it on another device names a
/// provider that device has no way to open, and shows up as a phantom account
/// asking to be reconnected.
///
/// The authority is `host_core::accounts::AdapterKind::is_host_internal`, which
/// cannot be reached from here — this crate sits below it. A test up there
/// asserts the two agree, so the copy cannot drift in silence.
pub const HOST_INTERNAL_ACCOUNT_KINDS: &[&str] = &["local", "device_calendar"];

/// Whether a row with this kind stays on the machine that made it.
pub fn is_host_internal_kind(adapter_kind: &str) -> bool {
    HOST_INTERNAL_ACCOUNT_KINDS.contains(&adapter_kind)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountPayload {
    /// Stable account id (UUID minted on the originating device).
    pub id: String,
    /// Adapter kind wire string (`caldav`, `ical`, `ews`,
    /// `vikunja`, `todoist`, `google`, `microsoft_graph`, …).
    pub adapter_kind: String,
    pub display_name: String,
    /// Adapter-specific non-secret config (server URL, username,
    /// OAuth client_id, …) as a JSON string. The applier stores
    /// it opaquely; the actual schema is owned by each adapter.
    pub config_json: String,
    /// RFC3339 timestamps from the originating device. Carried
    /// so the row on the receiving device matches what a future
    /// snapshot apply would produce.
    pub created_at: String,
    pub updated_at: String,
}

/// Payload for `credential.set`. Carries the **plaintext secret** for one
/// `(account_id, slot)` pair — which is why this payload is only ever
/// produced while E2E is on, so it lives exclusively inside an encrypted
/// log blob. `slot` is the secret-slot wire name (`password`,
/// `refresh_token`, `api_token`); the applier maps it back to a keychain
/// slot. Short-lived `access_token`s are deliberately NOT synced — each
/// device re-derives its own from the refresh token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialPayload {
    pub account_id: String,
    pub slot: String,
    pub secret: String,
}

/// Payload for `credential.cleared` — the account + slot, no secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialSlotPayload {
    pub account_id: String,
    pub slot: String,
}

/// Payload for `shortcut.set` — the action key plus the new
/// binding. Keys come from the canonical action catalogue
/// (`src/shortcuts.ts` etc.); the binding is the platform-neutral
/// representation ("Mod+S", "Alt+ArrowRight"). Conflict detection
/// happens client-side when the user accepts the change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutPayload {
    pub action: String,
    pub binding: String,
}

/// Payload for `shortcut.reset` and `shortcut.cleared` — the action
/// key alone, no new binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutKeyPayload {
    pub action: String,
}

/// Payload for `settings.updated`. The wire shape carries the
/// setting key (in the `user_prefs.<dotted>` style the app already
/// uses) and the new value as untyped JSON — a sound config object,
/// a string, a number, whatever the setting holds. The applier
/// validates against the §19.2.1 whitelist before writing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsPayload {
    pub key: String,
    pub value: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_device_id() -> DeviceId {
        // Stable id so the assertion strings stay readable.
        DeviceId::from_string("device-test".into())
    }

    #[test]
    fn event_created_round_trips_through_json() {
        let env = EventEnvelope {
            id: "evt_test".into(),
            device_id: fixture_device_id(),
            timestamp: chrono::DateTime::parse_from_rfc3339("2025-05-12T09:14:22.341Z")
                .unwrap()
                .with_timezone(&Utc),
            event: SyncEvent::EventCreated(EventPayload {
                id: "cal_evt_abc123".into(),
                fields: serde_json::json!({
                    "title": "Teammeeting",
                    "start": "2025-05-15T10:00:00Z",
                }),
            }),
        };
        let encoded = serde_json::to_string(&env).unwrap();
        // The serde flatten contract puts `type` at the top level.
        assert!(encoded.contains(r#""type":"event.created""#));
        let decoded: EventEnvelope = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, env);
    }

    #[test]
    fn task_updated_carries_a_partial_diff() {
        let env = EventEnvelope::new(
            fixture_device_id(),
            SyncEvent::TaskUpdated(EventPayload {
                id: "task-1".into(),
                // Only the touched fields.
                fields: serde_json::json!({ "status": "completed" }),
            }),
        );
        let encoded = serde_json::to_string(&env).unwrap();
        assert!(encoded.contains(r#""type":"task.updated""#));
        assert!(encoded.contains(r#""status":"completed""#));
        // The untouched fields (title, deadline, etc.) are NOT in
        // the payload — the applier will merge against the
        // existing row server-side.
        assert!(!encoded.contains("\"title\""));
    }

    #[test]
    fn deleted_carries_only_id() {
        let env = EventEnvelope::new(
            fixture_device_id(),
            SyncEvent::EventDeleted(IdPayload {
                id: "ev-gone".into(),
            }),
        );
        let encoded = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        // payload should be `{ "id": "ev-gone" }` — no leftover
        // empty-fields keys.
        assert_eq!(parsed["payload"]["id"], "ev-gone");
        assert!(parsed["payload"].get("fields").is_none());
    }

    #[test]
    fn settings_updated_carries_typed_value() {
        let env = EventEnvelope::new(
            fixture_device_id(),
            SyncEvent::SettingsUpdated(SettingsPayload {
                key: "appearance.darkMode".into(),
                value: serde_json::json!(true),
            }),
        );
        let encoded = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(parsed["type"], "settings.updated");
        assert_eq!(parsed["payload"]["key"], "appearance.darkMode");
        assert_eq!(parsed["payload"]["value"], true);
    }

    #[test]
    fn shortcut_set_keeps_action_and_binding() {
        let env = EventEnvelope::new(
            fixture_device_id(),
            SyncEvent::ShortcutSet(ShortcutPayload {
                action: "event.save".into(),
                binding: "Mod+S".into(),
            }),
        );
        let encoded = serde_json::to_string(&env).unwrap();
        assert!(encoded.contains(r#""action":"event.save""#));
        assert!(encoded.contains(r#""binding":"Mod+S""#));
    }

    #[test]
    fn plugin_uninstalled_uses_id_payload() {
        let env = EventEnvelope::new(
            fixture_device_id(),
            SyncEvent::PluginUninstalled(IdPayload {
                id: "io.example.timeline".into(),
            }),
        );
        let encoded = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(parsed["type"], "plugin.uninstalled");
        assert_eq!(parsed["payload"]["id"], "io.example.timeline");
    }

    #[test]
    fn every_variant_round_trips() {
        // Smoke check — touch every variant once so a future serde
        // attribute change doesn't quietly break encoding for the
        // ones we don't otherwise exercise.
        let dev = fixture_device_id();
        let payloads: Vec<SyncEvent> = vec![
            SyncEvent::EventCreated(EventPayload {
                id: "1".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::EventUpdated(EventPayload {
                id: "1".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::EventDeleted(IdPayload { id: "1".into() }),
            SyncEvent::TaskCreated(EventPayload {
                id: "2".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::TaskUpdated(EventPayload {
                id: "2".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::TaskDeleted(IdPayload { id: "2".into() }),
            SyncEvent::TaskListCreated(EventPayload {
                id: "3".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::TaskListUpdated(EventPayload {
                id: "3".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::TaskListDeleted(IdPayload { id: "3".into() }),
            SyncEvent::SectionCreated(EventPayload {
                id: "s1".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::SectionUpdated(EventPayload {
                id: "s1".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::SectionDeleted(IdPayload { id: "s1".into() }),
            SyncEvent::CalendarCreated(EventPayload {
                id: "4".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::CalendarUpdated(EventPayload {
                id: "4".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::CalendarDeleted(IdPayload { id: "4".into() }),
            SyncEvent::ColorLabelCreated(EventPayload {
                id: "5".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::ColorLabelUpdated(EventPayload {
                id: "5".into(),
                fields: serde_json::json!({}),
            }),
            SyncEvent::ColorLabelDeleted(IdPayload { id: "5".into() }),
            SyncEvent::PluginInstalled(PluginPayload {
                id: "p".into(),
                version: "1.0.0".into(),
                source: Some("registry://x".into()),
                name: None,
                plugin_type: None,
            }),
            SyncEvent::PluginUpdated(PluginPayload {
                id: "p".into(),
                version: "1.0.1".into(),
                source: None,
                name: Some("Plugin name".into()),
                plugin_type: Some("calendar-adapter".into()),
            }),
            SyncEvent::PluginUninstalled(IdPayload { id: "p".into() }),
            SyncEvent::AccountCreated(AccountPayload {
                id: "acc-1".into(),
                adapter_kind: "caldav".into(),
                display_name: "Work".into(),
                config_json: "{\"server_url\":\"https://dav.example.com\"}".into(),
                created_at: "2025-05-12T09:14:22.341Z".into(),
                updated_at: "2025-05-12T09:14:22.341Z".into(),
            }),
            SyncEvent::AccountUpdated(AccountPayload {
                id: "acc-1".into(),
                adapter_kind: "caldav".into(),
                display_name: "Work (renamed)".into(),
                config_json: "{\"server_url\":\"https://dav.example.com\"}".into(),
                created_at: "2025-05-12T09:14:22.341Z".into(),
                updated_at: "2025-05-12T09:20:00.000Z".into(),
            }),
            SyncEvent::AccountDeleted(IdPayload { id: "acc-1".into() }),
            SyncEvent::CredentialSet(CredentialPayload {
                account_id: "acc-1".into(),
                slot: "password".into(),
                secret: "hunter2".into(),
            }),
            SyncEvent::CredentialCleared(CredentialSlotPayload {
                account_id: "acc-1".into(),
                slot: "password".into(),
            }),
            SyncEvent::ShortcutSet(ShortcutPayload {
                action: "x".into(),
                binding: "Mod+X".into(),
            }),
            SyncEvent::ShortcutReset(ShortcutKeyPayload { action: "y".into() }),
            SyncEvent::ShortcutCleared(ShortcutKeyPayload { action: "z".into() }),
            SyncEvent::SettingsUpdated(SettingsPayload {
                key: "k".into(),
                value: serde_json::json!(42),
            }),
        ];
        for payload in payloads {
            let env = EventEnvelope::new(dev.clone(), payload);
            let encoded = serde_json::to_string(&env).unwrap();
            let decoded: EventEnvelope = serde_json::from_str(&encoded).unwrap();
            assert_eq!(env, decoded);
        }
    }

    #[test]
    fn mint_event_id_is_unique_across_calls() {
        // Two consecutive mints must differ — the random tail
        // covers the collision-during-same-millisecond case.
        let a = mint_event_id();
        let b = mint_event_id();
        assert_ne!(a, b);
        assert!(a.starts_with("evt_"));
        assert!(b.starts_with("evt_"));
    }
}
