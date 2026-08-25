// TypeScript counterparts to the cal-core types.
//
// Phase 1 maintains these by hand. The crate `cal-core` is the source of
// truth; if a field changes there, mirror it here. Phase 2 will look at
// generating these from the Rust definitions (e.g. via `specta`), at
// which point this file becomes a starting point.
//
// The task domain (Task, TaskList, Section, plus the cross-cutting
// ContainerColor / Reminder / SoundConfig / ColorLabel / recurrence-capability
// types) now lives in the shared `@aperio/shared` package — reused by the
// mobile app — and is re-exported here so existing `src/api/types` imports keep
// resolving unchanged.

import type {
  ContainerColor,
  RecurrenceCapabilities,
  Reminder,
  SoundConfig,
  Task,
  WireContactValue,
} from '@aperio/shared';

export type * from '@aperio/shared';

export interface Calendar {
  id: string;
  name: string;
  color: ContainerColor | null;
  /** Bound color-label id. When set, the rendered color resolves to the
   *  label's live hex (priority over `color`). See `resolveContainerColor`. */
  color_label: string | null;
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
  /** True when the owning provider can email attendees about
   *  invitations/updates/cancellations via server-side scheduling (EWS,
   *  Google, Graph always; CalDAV/iCloud only when the server advertises
   *  RFC 6638). Gates the "notify attendees" toggle in the event dialog.
   *  Absent → treat as unsupported. */
  supports_scheduling?: boolean;
  /** True when the owning provider stores a per-event color natively
   *  (RFC 7986 COLOR): local always; color-capable CalDAV (non-iCloud);
   *  Google/Graph/EWS/iCal never. When false the color is kept as a
   *  host-local override instead. Routes a recolor through `update_event`
   *  (native) vs `setEventColor` (override). Absent → treat as unsupported. */
  supports_event_color?: boolean;
}

export interface EventRecurrence {
  rrule: string;
  exceptions: string[];
  /** IANA zone of the master DTSTART, when the source carried one; drives
   *  DST-correct expansion in `@aperio/shared` recurrence.ts. */
  tzid?: string | null;
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
  /** Read-only native color (`#rrggbb`, RFC 7986 COLOR) from a color-capable
   *  provider. The host maps it onto `color_label` when it matches a known
   *  label; otherwise it stays here and `resolveEventColor` renders it directly
   *  (a subscribed feed's color, or a foreign color another client set). Never
   *  send it back. */
  color_hex?: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
  /** Transient organizer-side send intent for the next update: when true,
   *  the adapter asks the provider to email attendees. Not persisted. */
  send_invitations?: boolean;
  /** Transient "this and all following" signal for the next update: when true on
   *  a truncated recurring MASTER, CalDAV/iCloud + Google adapters drop any
   *  RECURRENCE-ID override in the dropped tail so it doesn't ghost. Not
   *  persisted; only the split's update sets it. */
  truncate_tail_overrides?: boolean;
  created_at: string;
  updated_at: string;
  etag: string | null;
  /** Organizer address (mailto: stripped) when the provider exposes it on
   *  read. Lets the UI tell whether the connected account is an *attendee*
   *  of this meeting rather than its organizer — the gate for RSVP. */
  organizer?: string | null;
  /** Per-attendee RSVP state, populated on read where the provider reports
   *  it. Read-only; absent/empty otherwise. */
  attendee_responses?: AttendeeResponse[];
  /** The meeting is cancelled (RFC 5545 STATUS:CANCELLED / EWS IsCancelled /
   *  Graph isCancelled). Read-only. Cancelled events never fire reminders and
   *  are hidden when the user turns off "show cancelled events". Absent/false
   *  for local events and providers that don't report cancellation. */
  cancelled?: boolean;
}

/** An attendee's RSVP status (RFC 5545 PARTSTAT), normalised across
 *  providers. Mirrors cal-core `AttendeeStatus` (serde kebab-case). */
export type AttendeeStatus =
  | 'needs-action'
  | 'accepted'
  | 'declined'
  | 'tentative';

/** One attendee's RSVP state on an event read from a provider. */
export interface AttendeeResponse {
  email: string;
  name?: string | null;
  status: AttendeeStatus;
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
  /** Organizer-side send intent for this create — see CalendarEvent. */
  send_invitations?: boolean;
}

/** One busy time block (ISO 8601 timestamps) from a free/busy query. */
export interface FreeBusySlot {
  start: string;
  end: string;
}

/** An attendee's busy blocks within the queried window. An empty `slots`
 *  array means "no known conflicts" (or the provider couldn't answer). */
export interface FreeBusy {
  email: string;
  slots: FreeBusySlot[];
}

/** Address book — the contacts equivalent of `Calendar`/`TaskList`.
 *  Same `account_id` enrichment so the sidebar can group by source. */
export interface ContactList {
  id: string;
  name: string;
  color: ContainerColor | null;
  /** Bound color-label id — see `Calendar.color_label`. */
  color_label: string | null;
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
  /** Honorific name prefix ("Prof. Dr.") and suffix ("jun.") — vCard `N`
   *  components 4/5, Google honorificPrefix/Suffix, Graph title/generation.
   *  EWS surfaces neither (read-only CompleteName), so they stay null there. */
  name_prefix: string | null;
  name_suffix: string | null;
  organization: string | null;
  /** See `WireContactValue`: an object with a label, or a bare string for
   *  anything stored before labels existed. Normalise with
   *  `toContactValues` from `@aperio/shared` before rendering. */
  emails: WireContactValue[];
  phone_numbers: WireContactValue[];
  /** Websites. Same labelled shape; CardDAV and Google carry several,
   *  Exchange and Graph exactly one. */
  urls: WireContactValue[];
  /** ISO date (YYYY-MM-DD) or null. */
  birthday: string | null;
  /** Wedding / partnership anniversary as an ISO date, or null. Not every
   *  provider has a field for it — Microsoft Graph notably does not. */
  anniversary: string | null;
  job_title: string | null;
  department: string | null;
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
  /** Honorific name prefix ("Prof. Dr.") and suffix ("jun.") — vCard `N`
   *  components 4/5, Google honorificPrefix/Suffix, Graph title/generation.
   *  EWS surfaces neither (read-only CompleteName), so they stay null there. */
  name_prefix: string | null;
  name_suffix: string | null;
  organization: string | null;
  /** See `Contact.emails`. Write with `fromContactValues`. */
  emails: WireContactValue[];
  phone_numbers: WireContactValue[];
  urls: WireContactValue[];
  birthday: string | null;
  anniversary: string | null;
  job_title: string | null;
  department: string | null;
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

/** One entry in the day-marker vocabulary, and one day's record.
 *
 *  Defined in `@aperio/shared` because both frontends resolve and summarise
 *  them with the same helpers; re-exported here so existing `src/api/types`
 *  imports keep working. */
export type { DayLog, DayMarker } from '@aperio/shared';

export interface SearchResults {
  events: CalendarEvent[];
  tasks: Task[];
}

/** Adapter kinds known to the backend. Phase 6a only allows `local`
 *  to be created; the others appear in the UI as "coming soon" until
 *  their respective adapter lands. */
/** Which adapter an account belongs to.
 *
 *  A plain string, deliberately. It used to be a closed union, which meant an
 *  adapter could not exist until this file was edited — and the frontend has no
 *  business holding the list at all: it is decided by which plugins are
 *  installed, which is a runtime fact. `listAdapterKinds()` asks the host.
 *
 *  The VALUES are unchanged and have to stay that way: this string is persisted
 *  in every account row and travels in every sync payload, so an older device
 *  matches on exactly these bytes.
 *
 *  The two host-internal kinds are the only ones the app knows by name, because
 *  neither is a plugin: `local` is the built-in store and `device_calendar` is
 *  built over a native bridge. */
export type AdapterKind = string;

/** The built-in local store. */
export const ADAPTER_KIND_LOCAL = 'local';
/** The device's own calendar + reminders (mobile only, never synced). */
export const ADAPTER_KIND_DEVICE_CALENDAR = 'device_calendar';

export interface Account {
  id: string;
  adapter_kind: AdapterKind;
  display_name: string;
  /** Adapter-specific non-secret config as a JSON string. */
  config_json: string;
  created_at: string;
  updated_at: string;
  /** Derived at list-time from the plugin's declared TYPE: whether this
   *  account can mint meetings. Drives the event editor's "create meeting"
   *  control without the UI knowing any provider names. */
  is_videoconference?: boolean;
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
    // Reconnecting a bring-your-own OAuth account on a device that does not
    // hold its client secret — the dialog asks for the secret and retries.
    | 'client_secret_required'
    // "Re-sync from scratch" on an account with no registered adapter: there
    // is nothing to wipe or fetch, and the honest answer is a refusal that
    // points at reconnecting.
    | 'not_registered'
    | 'encryption_required'
    // A key this device already held, tried against the target's dataset and
    // refused by it. Distinct from `decryption_failed`, which is the same
    // wrongness discovered mid-round, after the choice was already made.
    | 'encryption_key_mismatch'
    | 'decryption_failed'
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
    | 'restart_required'
    // §19.5 — `select_sync_account` refuses an account whose protocol pins
    // host keys until this device has confirmed the server's fingerprint.
    // Its own code because the repair is its own gesture: the sync settings
    // offer the trust dialog rather than a message the user cannot act on.
    | 'host_key_not_trusted'
    // `group_events` refuses to merge two existing groups. Its own code
    // because the answer is a sentence only the user can act on ("take one of
    // them out first"), not a failure to report.
    | 'event_group_conflict'
    // A group needs at least two events. The UI never sends fewer, so this is
    // a guard rather than a message anyone should meet.
    | 'event_group_too_few';
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

/** One container whose most recent background refresh FAILED — the raw
 *  material of the per-account error surface (silent staleness, e.g. a
 *  revoked iCloud app password). Mirrors host-core
 *  `cache::ContainerRefreshError`. */
export interface ContainerRefreshError {
  /** SyncScope wire string: "events" | "tasks" | "sections" | "contacts"
   *  or a listing scope ("calendars" | "task_lists" | "contact_lists"). */
  scope: string;
  /** Container id, or "" for an account-level listing failure. */
  container_id: string;
  /** Resolved container name, when the cached listing knows it. */
  container_name: string | null;
  /** The recorded provider error text. */
  error: string;
  /** Last SUCCESSFUL refresh (RFC 3339) — how stale the visible data is.
   *  `null`: never refreshed successfully. */
  last_success_at: string | null;
}

/** Every failing container of one account, plus the auth heuristic that
 *  drives the "re-enter password" hint. */
export interface AccountRefreshErrors {
  account_id: string;
  auth_suspected: boolean;
  errors: ContainerRefreshError[];
}
