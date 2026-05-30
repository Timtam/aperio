// TypeScript counterparts to the cal-core types.
//
// Phase 1 maintains these by hand. The crate `cal-core` is the source of
// truth; if a field changes there, mirror it here. Phase 2 will look at
// generating these from the Rust definitions (e.g. via `specta`), at
// which point this file becomes a starting point.

export interface ContainerColor {
  hex: string;
  source: 'native' | 'custom';
}

/** Which recurrence shapes the owning adapter can store. Mirrors
 *  `plugin_core::RecurrenceCapabilities`; stamped onto every
 *  Calendar by the backend (resolved from the account's plugin
 *  manifest). The EventDialog greys out options the source can't
 *  round-trip — e.g. EWS omits `"yearly"` from `interval_frequencies`
 *  because Exchange has no yearly interval. Local + unknown sources
 *  report full RFC-5545 support. */
export interface RecurrenceCapabilities {
  frequencies: RecurrenceFreq[];
  interval_frequencies: RecurrenceFreq[];
  relative_monthly: boolean;
  relative_yearly: boolean;
  weekly_byday: boolean;
  count: boolean;
  until: boolean;
}

export type RecurrenceFreq = 'daily' | 'weekly' | 'monthly' | 'yearly';

/** Which task-organisation features the owning adapter supports.
 *  Mirrors `plugin_core::TaskCapabilities`; stamped onto every
 *  `TaskList` by the backend (resolved from the account's plugin
 *  manifest, or the local store's hard-coded set). The task UI gates
 *  affordances on these — e.g. only offering "add section" where
 *  `sections` is true, or cross-project drag where
 *  `move_between_projects` is true. Absent → treat as the cal-core-
 *  native default (flat lists, single-level subtasks, cross-list move).*/
export interface TaskCapabilities {
  nested_projects: boolean;
  subtasks: boolean;
  /** `null` ⇒ unlimited nesting depth. */
  max_subtask_depth: number | null;
  sections: boolean;
  multiple_labels: boolean;
  task_recurrence: boolean;
  move_between_projects: boolean;
  /** The adapter can create new task lists (projects) at the source.
   *  The sidebar offers "new list in this account" only where true. */
  create_lists: boolean;
  /** The adapter can delete task lists at the source. */
  delete_lists: boolean;
}

/** A sub-grouping of tasks within one list — a Vikunja bucket or a
 *  Todoist section. Mirrors `cal_core::Section`. */
export interface Section {
  id: string;
  list_id: string;
  name: string;
  /** Display order within the list; lower sorts first. */
  order: number;
}

export interface Calendar {
  id: string;
  name: string;
  color: ContainerColor | null;
  read_only: boolean;
  default_sound: SoundConfig | null;
  /** Account that owns this calendar. `"local"` for the implicit
   *  local adapter; a UUID for any external account. Backend
   *  enriches every Calendar with this field via the registry's
   *  route map so the frontend can group containers by source
   *  without a second round-trip. */
  account_id: string;
  /** Recurrence shapes the owning adapter can store. Backend-
   *  stamped alongside `account_id`; optional in the wire shape so
   *  any consumer reading a Calendar from a pre-capabilities
   *  snapshot still parses. Absent → treat as full support. */
  recurrence_capabilities?: RecurrenceCapabilities;
}

export interface SoundConfig {
  source:
    | { type: 'system' }
    | { type: 'silent' }
    | { type: 'custom'; sha256: string };
  volume: number;
}

export interface Reminder {
  kind:
    | { type: 'relative'; minutes_before: number }
    | { type: 'absolute'; at: string }
    | { type: 'app_start' }
    | { type: 'email'; minutes_before: number };
  sound: SoundConfig | null;
}

export interface EventRecurrence {
  rrule: string;
  exceptions: string[];
}

export interface CalendarEvent {
  id: string;
  calendar_id: string;
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
  created_at: string;
  updated_at: string;
  etag: string | null;
}

export interface NewEvent {
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
}

export interface TaskList {
  id: string;
  name: string;
  color: ContainerColor | null;
  default_sound: SoundConfig | null;
  embedded_in_calendar: string | null;
  read_only: boolean;
  /** Account that owns this task list. Same semantics as
   *  `Calendar.account_id` — populated by the backend's route map. */
  account_id: string;
  /** Parent project id for backends with nested projects (Vikunja,
   *  Todoist). `null` for top-level lists and flat backends. Refers to
   *  another `TaskList.id` owned by the same account. The sidebar
   *  builds its project tree from this. */
  parent_id: string | null;
  /** Task-organisation capabilities of the owning adapter. Backend-
   *  stamped alongside `account_id`; optional in the wire shape so a
   *  consumer reading a list from a pre-capabilities snapshot still
   *  parses. Absent → cal-core-native default. */
  task_capabilities?: TaskCapabilities;
}

export type TaskStatus = 'open' | 'in_progress' | 'completed' | 'cancelled';
export type TaskPriority = 'low' | 'medium' | 'high';

export interface Task {
  id: string;
  list_id: string;
  title: string;
  description: string | null;
  status: TaskStatus;
  priority: TaskPriority;
  /**
   * The day the task is to be done. Was historically also where the
   * old `deadline_type='on'` cases lived after migration 0006 — the
   * "Geplanter Tag" and "Konkrete Deadline" of the old enum are now
   * one and the same field.
   */
  scheduled_date: string | null;
  /**
   * Optional time-of-day on `scheduled_date`. Renders as a point
   * marker in the day grid (no block duration). Requires
   * `scheduled_date`; the DB enforces this via a CHECK constraint.
   */
  scheduled_time: string | null;
  /**
   * The day BY which the task must be done. The only deadline
   * semantic that survives migration 0006 — what used to be
   * `deadline_type='by'`. Until that day the task lives in the
   * backlog and can be scheduled per-day via `scheduled_date`; on
   * the deadline day, if still open, an app-start checker pins it
   * to today.
   */
  deadline_date: string | null;
  deadline_time: string | null;
  recurrence: unknown;
  parent_id: string | null;
  /** Section (Vikunja bucket / Todoist section) this task is filed
   *  under within its list. `null` ⇒ ungrouped, or a backend with no
   *  sections. Refers to a `Section.id` whose `list_id == this.list_id`. */
  section_id: string | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  etag: string | null;
}

export interface ColorLabel {
  id: string;
  name: string;
  hex: string;
}

/** Address book — the contacts equivalent of `Calendar`/`TaskList`.
 *  Same `account_id` enrichment so the sidebar can group by source. */
export interface ContactList {
  id: string;
  name: string;
  color: ContainerColor | null;
  read_only: boolean;
  account_id: string;
}

/** One row inside a distribution list. Matches the cal-core
 *  `GroupMember` type. The picker needs an email to be useful;
 *  the name is optional and survives round-trips through
 *  EWS (`<t:Mailbox>`) and vCard (`MEMBER;CN=…`). */
export interface GroupMember {
  name: string | null;
  email: string;
}

export interface Contact {
  id: string;
  list_id: string;
  display_name: string;
  given_name: string | null;
  family_name: string | null;
  organization: string | null;
  emails: string[];
  phone_numbers: string[];
  /** ISO date (YYYY-MM-DD) or null. */
  birthday: string | null;
  notes: string | null;
  /** Distribution-list membership marker. `null` ⇒ regular
   *  person-contact (the default). A non-null array (possibly
   *  empty) ⇒ this row is a group / distribution list.
   *  Aperio surfaces the distinction in the dialog (member
   *  editor) and in the list view (group badge). */
  members: GroupMember[] | null;
  /** Avatar presence flag (Phase 10g). `true` ⇒ the contact has
   *  a photo we can fetch via `get_contact_photo`; the listing
   *  doesn't carry the bytes themselves so a thousand-row pull
   *  stays cheap. The dialog and the attendees picker probe
   *  `has_photo` to decide whether to issue a follow-up fetch
   *  or render the initials placeholder. */
  has_photo: boolean;
  /** Postal addresses (Phase 10l / vCard ADR). One entry per
   *  address; empty list ⇒ no postal addresses on record. */
  addresses: ContactAddress[];
  created_at: string;
  updated_at: string;
  etag: string | null;
}

/** One postal address attached to a contact. Wire shape matches
 *  cal_core::ContactAddress — every field optional because vCard
 *  ADR allows arbitrary subsets and the four adapters round-trip
 *  the same flexibility. */
export interface ContactAddress {
  /** `"home"` / `"work"` / `"other"` — free-form for forward
   *  compatibility with custom vCard 4.0 TYPE values. */
  label?: string | null;
  street?: string | null;
  city?: string | null;
  region?: string | null;
  postal_code?: string | null;
  country?: string | null;
}

/** Avatar bytes attached to a contact. Round-tripped on
 *  `get_contact_photo` / `set_contact_photo`, with `data` carried
 *  as a base64-encoded string (the Rust side custom-serdes the
 *  `Vec<u8>` so the JSON shape stays small). */
export interface ContactPhoto {
  content_type: string;
  /** Base64-encoded image bytes. */
  data: string;
}

/** Payload for `create_contact` — server fills in id + timestamps. */
export interface NewContact {
  display_name: string;
  given_name: string | null;
  family_name: string | null;
  organization: string | null;
  emails: string[];
  phone_numbers: string[];
  birthday: string | null;
  notes: string | null;
  /** See `Contact.addresses`. Empty array ⇒ no postal addresses. */
  addresses: ContactAddress[];
  /** See `Contact.members`. `null` ⇒ create a person; a non-null
   *  array (possibly empty) ⇒ create a distribution list. */
  members: GroupMember[] | null;
  /** Optional inline avatar — backend writes it as part of the
   *  create round-trip so a "new contact with photo" gesture
   *  lands as one command. `null` for the common case. */
  photo: ContactPhoto | null;
}

export interface SearchResults {
  events: CalendarEvent[];
  tasks: Task[];
}

/** Adapter kinds known to the backend. Phase 6a only allows `local`
 *  to be created; the others appear in the UI as "coming soon" until
 *  their respective adapter lands. */
export type AdapterKind =
  | 'local'
  | 'caldav'
  | 'ical'
  | 'google'
  | 'microsoft_graph'
  | 'ews'
  | 'vikunja'
  | 'todoist'
  | 'zoom'
  | 'teams'
  | 'meet'
  | 'webex';

export interface Account {
  id: string;
  adapter_kind: AdapterKind;
  display_name: string;
  /** Adapter-specific non-secret config as a JSON string. */
  config_json: string;
  created_at: string;
  updated_at: string;
  /** Derived at list-time by the backend: `true` when this
   *  account's `adapter_kind` maps to a plugin id that's
   *  currently loaded + enabled in the host's PluginManager.
   *  `false` triggers the §20.8 "Plugin fehlt" indicator on
   *  the AccountsPanel row. Local accounts always return
   *  `true` (host-internal, no plugin to resolve).
   *
   *  Optional in the wire shape so legacy `list_accounts`
   *  consumers that read this struct outside the panel
   *  context (e.g. snapshot serialisation) don't trip. */
  plugin_loaded?: boolean;
}

export interface CommandError {
  code:
    | 'auth'
    | 'forbidden'
    | 'not_found'
    | 'conflict'
    | 'network'
    | 'protocol'
    | 'invalid_input'
    | 'unsupported'
    | 'internal'
    // Phase Sd onwards — cross-device sync error codes. The Rust
    // side maps `SyncError` variants into these so the frontend can
    // pattern-match on a stable string.
    | 'io'
    | 'encryption_required'
    | 'schema_too_old'
    | 'not_configured'
    // §20 plugin-system surface — `list_plugins` and friends
    // emit these when a plugin is missing from the
    // `PluginManager` or doesn't export a required entry point.
    | 'plugin_missing'
    // `set_plugin_enabled` refuses to disable the sync plugin
    // the user is currently configured to sync with — letting
    // them through would silently break every subsequent sync
    // round.
    | 'active_sync_conflict'
    // `install_plugin_archive` refuses an in-place upgrade
    // when the plugin id is already loaded — v1 needs a
    // restart to swap libraries safely.
    | 'restart_required';
  message: string;
}

/** Snapshot of one loaded plugin from `list_plugins`. Mirrors
 *  `src-tauri/src/commands/plugins.rs::PluginInfo`. */
export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  plugin_type: PluginTypeWire;
  capabilities: string[];
  abi_version: number;
  min_app_version: string;
  author: string | null;
  description: string | null;
  signed: boolean;
  has_interactive_auth: boolean;
  has_discover: boolean;
  has_probe_host_key: boolean;
  /** `true` when the plugin is currently routable; `false`
   *  when the user has toggled it off via the Settings panel.
   *  The cdylib stays loaded either way — the host's
   *  PluginManager.get() acts as if a disabled plugin weren't
   *  installed. */
  enabled: boolean;
  /** Where the plugin lives on disk. `"bundled"` plugins ship
   *  with the app and CANNOT be uninstalled; `"user"` plugins
   *  were installed via the §20.7 `.aperio` flow and have an
   *  Uninstall button in the panel. */
  source: 'bundled' | 'user';
}

/** A plugin directory the host's PluginManager refused to
 *  load at startup. Returned by `list_failed_plugins`;
 *  PluginsPanel renders these as the "Konnten nicht geladen
 *  werden"-section so stale community plugins after an
 *  Aperio update aren't invisible to the user. */
export interface FailedPluginInfo {
  plugin_dir: string;
  /** Manifest fields, populated when plugin.json parsed
   *  successfully (most failure modes apart from JSON parse
   *  errors). `null` for parse-time failures — the panel
   *  falls back to `plugin_dir`'s basename. */
  id: string | null;
  name: string | null;
  version: string | null;
  plugin_type: string | null;
  author: string | null;
  reason: FailedPluginReason;
  error_message: string;
}

/** Discriminated reason for the failure. The panel branches
 *  on `kind` to render an actionable hint per type. */
export type FailedPluginReason =
  | { kind: 'abi_mismatch'; host: number; plugin: number }
  | { kind: 'app_too_old'; required: string; running: string }
  | { kind: 'manifest_invalid' }
  | { kind: 'library_load' }
  | { kind: 'other' };

/** A plugin announcement another device has emitted into
 *  the shared Event Log (DESIGN.md §20.8) that THIS device
 *  doesn't have installed locally. Returned by
 *  `list_remote_plugins` + rendered as the "Plugin benötigt"
 *  section in the Settings → Plugins panel. */
export interface RemotePluginAnnouncement {
  id: string;
  /** May be null when the announcement came from a
   *  pre-iteration-21 Aperio (the optional `name` field
   *  wasn't part of the payload back then). */
  name: string | null;
  version: string;
  plugin_type: string | null;
  source: string | null;
  announced_by_device: string;
  /** Resolved name of the announcing device — populated by
   *  the host from meta.json's DeviceRecord. May be `null`
   *  when we haven't yet synced a meta.json that names the
   *  device; the UI falls back to `announced_by_device` in
   *  that case. */
  announced_by_device_name: string | null;
  /** RFC 3339. */
  announced_at: string;
}

/** Preview of a `.aperio` archive's manifest, returned by
 *  `inspect_plugin_archive`. Mirrors
 *  `src-tauri/src/commands/plugins.rs::PluginArchivePreview`. */
export interface PluginArchivePreview {
  id: string;
  name: string;
  version: string;
  plugin_type: PluginTypeWire;
  capabilities: string[];
  abi_version: number;
  min_app_version: string;
  author: string | null;
  description: string | null;
  signed: boolean;
  /** `true` when a plugin with the same id is already
   *  loaded. The install dialog rephrases itself as an
   *  update prompt + shows the currently-installed version
   *  next to the incoming one. */
  already_installed: boolean;
  /** Currently-installed version, or `null` when
   *  [`Self::already_installed`] is false. */
  installed_version: string | null;
}

/** Canonical plugin-type wire strings — Rust's `PluginType::as_str`
 *  emits these. Unknown future tags round-trip as plain strings;
 *  the panel renders them under "Other". */
export type PluginTypeWire =
  | 'calendar-adapter'
  | 'sync-adapter'
  | 'videoconference-adapter'
  | 'notification'
  | string;
