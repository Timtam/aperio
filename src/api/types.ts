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
    | 'plugin_missing';
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
