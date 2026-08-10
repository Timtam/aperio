// Typed wrappers around the Tauri `invoke` boundary.
//
// Every function maps 1:1 to a `#[tauri::command]` defined in
// `src-tauri/src/commands/`. The argument shapes mirror the Rust
// `Request` structs.

import { invoke } from '@tauri-apps/api/core';
import { withCreatedRecurrenceZone } from '../intl/recurrence';
import type {
  Account,
  AdapterKind,
  AttendeeStatus,
  Calendar,
  CalendarEvent,
  ColorLabel,
  CommandError,
  Contact,
  ContactList,
  ContactPhoto,
  FailedPluginInfo,
  FreeBusy,
  MemberRight,
  NewContact,
  NewEvent,
  PluginArchivePreview,
  PluginInfo,
  RemotePluginAnnouncement,
  SearchResults,
  Section,
  SoundConfig,
  Task,
  TaskList,
  TaskListShare,
  TaskUser,
  AccountRefreshErrors,
} from './types';

/** Type guard — a backend error always carries `code` and `message`. */
export function isCommandError(value: unknown): value is CommandError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'code' in value &&
    'message' in value
  );
}

/** Open a URL in the OS default browser / mail client. The backend
 *  re-validates the scheme (only http/https/mailto) before handing it
 *  to the OS — descriptions come from untrusted sources, so this is
 *  never a raw shell open. Rejected URLs surface as a CommandError. */
export async function openExternalUrl(url: string): Promise<void> {
  await invoke('open_external_url', { url });
}

// ── Calendars ──────────────────────────────────────────────────────────────

export const listCalendars = () => invoke<Calendar[]>('list_calendars');

export interface CreateCalendarRequest {
  name: string;
  /** Bind the new calendar's color to this color-label id (or null). */
  color_label?: string | null;
}

export const createCalendar = (request: CreateCalendarRequest) =>
  invoke<Calendar>('create_calendar', { request });

export const deleteCalendar = (id: string) =>
  invoke<void>('delete_calendar', { id });

// ── Events ─────────────────────────────────────────────────────────────────

export interface EventRangeRequest {
  calendar_id: string;
  start: string;
  end: string;
}

export const getEvents = (request: EventRangeRequest) =>
  invoke<CalendarEvent[]>('get_events', { request });

export interface CreateEventRequest extends NewEvent {
  calendar_id: string;
}

export const createEvent = (
  request: CreateEventRequest,
  opts: { preserveRecurrenceZone?: boolean } = {},
) =>
  invoke<CalendarEvent>('create_event', {
    // Stamp the host's local zone onto a brand-new timed recurring rule so it
    // expands DST-correctly (parity with zoned CalDAV series); no-op otherwise.
    //
    // `preserveRecurrenceZone` skips that defaulting: a series-SPLIT tail
    // ("edit this and all following") is a CONTINUATION, not a fresh series, so
    // it must keep the master's zone VERBATIM — including a floating (null) zone.
    // The truncated head (updateEvent, never stamped) and this tail then expand
    // identically, so untouched occurrences never drift an hour across a DST
    // boundary; stamping only the tail would diverge the two halves.
    request: {
      ...request,
      recurrence: opts.preserveRecurrenceZone
        ? request.recurrence
        : withCreatedRecurrenceZone(request.recurrence, request.all_day),
    },
  });

/** Update an event in place, OR move it to a different calendar.
 *
 *  When the calendar field on the dialog stays unchanged this is a
 *  plain in-place update: the backend issues one PUT (CalDAV) /
 *  PATCH (Graph) / UPDATE (local SQLite) against the existing
 *  resource. When the user picks a different calendar, the backend
 *  routes via a create-on-target + delete-from-source path instead
 *  — a single PUT to the new resource URL with the old ETag's
 *  If-Match would always 412 because the target resource doesn't
 *  yet exist (we hit exactly that on iCloud → iCloud moves before
 *  this hint was wired through).
 *
 *  Callers pass `previousCalendarId` whenever they captured the
 *  event's original location (EventDialog reads it from the loaded
 *  event; MoveCopyDialog has it explicitly). Omit it for true
 *  in-place updates where there's no candidate "previous" — the
 *  AgendaView drag handlers, recurrence-occurrence edits, etc. */
export const updateEvent = (
  event: CalendarEvent,
  previousCalendarId?: string,
) =>
  invoke<CalendarEvent>('update_event', {
    event,
    previousCalendarId: previousCalendarId ?? null,
  });

/** Delete an event. `calendarId` is optional but recommended — the
 *  backend uses it to route the delete to the right adapter when the
 *  event lives on an external account. Omitting it falls back to
 *  "assume local", which is only correct for events the user just
 *  created locally. */
export const deleteEventById = (
  id: string,
  calendarId?: string,
  sendCancellations?: boolean,
) =>
  invoke<void>('delete_event', {
    id,
    calendarId: calendarId ?? null,
    sendCancellations: sendCancellations ?? null,
  });

/** Fetch a single event by id. Pass the owning `calendarId` so an EXTERNAL
 *  event resolves via the SWR snapshot cache (the local store has no row for it);
 *  omitting it, or a local calendar, reads the stored row. `id` is the series
 *  master id for a recurring event. */
export const getEventById = (id: string, calendarId?: string) =>
  invoke<CalendarEvent | null>('get_event_by_id', {
    id,
    calendarId: calendarId ?? null,
  });

/** Look up attendee availability through the account that owns `calendarId`.
 *  Best-effort: returns `[]` for local calendars or when the provider can't
 *  answer (no error). `rangeStart`/`rangeEnd` are ISO 8601 strings. */
export const queryFreeBusy = (
  calendarId: string,
  emails: string[],
  rangeStart: string,
  rangeEnd: string,
) =>
  invoke<FreeBusy[]>('query_free_busy', {
    request: {
      calendar_id: calendarId,
      emails,
      range_start: rangeStart,
      range_end: rangeEnd,
    },
  });

/** The connected account's email for `calendarId`, or null when the
 *  provider can't report one (local/iCal, or an identity lookup that
 *  failed). Drives the RSVP gate: the user can respond only to a meeting
 *  they're an attendee (not the organizer) of. */
export const calendarCurrentUserEmail = (calendarId: string) =>
  invoke<string | null>('calendar_current_user_email', { calendarId });

/** RSVP to an invitation: set the connected user's participation status
 *  on the event. When `sendResponse` is true the provider also emails the
 *  reply to the organizer. Only valid on scheduling-capable external
 *  calendars where the user is a (non-organizer) attendee. */
export const respondToEvent = (
  calendarId: string,
  eventId: string,
  status: AttendeeStatus,
  sendResponse: boolean,
) =>
  invoke<void>('respond_to_event', {
    calendarId,
    eventId,
    status,
    sendResponse,
  });

export const getTaskById = (id: string) =>
  invoke<Task | null>('get_task_by_id', { id });

/** Append `occurrence` to a recurring event's EXDATE list. Used when
 *  the user deletes or overrides a single occurrence — the master row
 *  stays intact and the expansion engine simply skips that date.
 *  `calendarId` lets the backend route the update to the right
 *  adapter (CalDAV / iCloud / local). It's optional only for
 *  backwards compatibility; new callers should always pass it. */
export const addEventExdate = (
  id: string,
  occurrence: string,
  calendarId?: string,
  sendCancellations?: boolean,
) =>
  invoke<void>('add_event_exdate', {
    id,
    occurrence,
    calendarId: calendarId ?? null,
    // Only the organizer's "cancel this occurrence + notify attendees" choice
    // passes true; the plain "delete only this occurrence" omits it.
    sendCancellations: sendCancellations ?? null,
  });

// ── Task lists & tasks ─────────────────────────────────────────────────────

export const listTaskLists = () => invoke<TaskList[]>('list_task_lists');

export interface CreateTaskListRequest {
  name: string;
  /** Account to create the list in. `null`/`"local"` ⇒ local store;
   *  otherwise routed to that account's adapter (needs create_lists). */
  account_id?: string | null;
  /** Parent list id for nesting (Vikunja/Todoist); `null` ⇒ top level. */
  parent_id?: string | null;
  embedded_in_calendar: string | null;
  /** Local-only: bind the new list's color to this color-label id. */
  color_label?: string | null;
}

export const createTaskList = (request: CreateTaskListRequest) =>
  invoke<TaskList>('create_task_list', { request });

export const deleteTaskList = (id: string) =>
  invoke<void>('delete_task_list', { id });

/** Reparent a local task list under `parentId` (or to the top level
 *  when `null`). Local-store only — see the backend command. */
export const reparentTaskList = (id: string, parentId: string | null) =>
  invoke<TaskList>('reparent_task_list', { request: { id, parent_id: parentId } });

export const getTasks = (list_id: string) =>
  invoke<Task[]>('get_tasks', { listId: list_id });

/** List the sections (Vikunja buckets / Todoist sections) of a list.
 *  Section-less backends return an empty array. */
export const getSections = (list_id: string) =>
  invoke<Section[]>('get_sections', { listId: list_id });

/** Users assignable to a task in this list — its collaborator pool
 *  (DESIGN §9.7). Empty for local lists / providers without sharing. */
export const taskListMembers = (list_id: string) =>
  invoke<TaskUser[]>('task_list_members', { listId: list_id });

/** The connected account's own identity ("me") for the account that
 *  owns this list. `null` for local lists / providers without users. */
export const taskCurrentUser = (list_id: string) =>
  invoke<TaskUser | null>('task_current_user', { listId: list_id });

// ── Task-list membership (DESIGN §9.7) ───────────────────────────────────────

/** Editable membership/shares of a list (with rights). Empty for local
 *  lists / non-manageable backends. */
export const taskListShares = (list_id: string) =>
  invoke<TaskListShare[]>('task_list_shares', { listId: list_id });

/** Directory search for users to add to a list (Vikunja). */
export const taskSearchUsers = (list_id: string, query: string) =>
  invoke<TaskUser[]>('task_search_users', { listId: list_id, query });

/** Add/invite a member. `memberRef` is the provider's add key
 *  (Vikunja username, Todoist email). */
export const taskAddMember = (
  list_id: string,
  member_ref: string,
  right: MemberRight | null,
) =>
  invoke<void>('task_add_member', {
    listId: list_id,
    memberRef: member_ref,
    right,
  });

export const taskRemoveMember = (list_id: string, member_ref: string) =>
  invoke<void>('task_remove_member', {
    listId: list_id,
    memberRef: member_ref,
  });

export const taskSetMemberRight = (
  list_id: string,
  member_ref: string,
  right: MemberRight,
) =>
  invoke<void>('task_set_member_right', {
    listId: list_id,
    memberRef: member_ref,
    right,
  });

export interface CreateSectionRequest {
  list_id: string;
  name: string;
  position: number;
  /** Optional color-label id for the new section. */
  color_label?: string | null;
}

/** Create a section. On local lists it's stored locally; on external lists
 *  whose provider declares `manageable_sections` (Todoist, Vikunja) the
 *  backend creates it at the provider. The section colour is always a local
 *  concern — set it separately via `setSectionColor`. */
export const createSection = (request: CreateSectionRequest) =>
  invoke<Section>('create_section', { request });

export const updateSection = (section: Section) =>
  invoke<Section>('update_section', { section });

export const deleteSection = (id: string, list_id: string) =>
  invoke<void>('delete_section', { id, listId: list_id });

export interface CreateTaskRequest {
  list_id: string;
  title: string;
  description: string | null;
  status: Task['status'];
  priority: Task['priority'];
  effort: Task['effort'];
  scheduled_date: string | null;
  scheduled_time: string | null;
  deadline_date: string | null;
  deadline_time: string | null;
  deadline_reminder_days: Task['deadline_reminder_days'];
  recurrence: unknown;
  parent_id: string | null;
  section_id: string | null;
  color_label: string | null;
  reminders: Task['reminders'];
  assignees: Task['assignees'];
  sound: Task['sound'];
}

export const createTask = (request: CreateTaskRequest) =>
  invoke<Task>('create_task', { request });

// ── Color labels ───────────────────────────────────────────────────────────

export const listColorLabels = () =>
  invoke<ColorLabel[]>('list_color_labels');

export interface CreateColorLabelRequest {
  name: string;
  hex: string;
}

export const createColorLabel = (request: CreateColorLabelRequest) =>
  invoke<ColorLabel>('create_color_label', { request });

/** Resolve a custom one-off color (hex) to a hidden, hex-deduplicated
 *  "ad-hoc" color label and return it. Re-picking the same hex reuses the
 *  same label. Used by the custom-color picker for quick (un-named) colors;
 *  named colors go through `createColorLabel`. */
export const getOrCreateAdHocColorLabel = (hex: string) =>
  invoke<ColorLabel>('get_or_create_ad_hoc_color_label', { hex });

export const updateColorLabel = (label: ColorLabel) =>
  invoke<ColorLabel>('update_color_label', { label });

export const deleteColorLabel = (id: string) =>
  invoke<void>('delete_color_label', { id });

// ── Search ─────────────────────────────────────────────────────────────────

export type SearchKind = 'both' | 'events' | 'tasks';

export type EventTypeFilter = 'any' | 'single' | 'recurring' | 'all_day';

export interface SearchFilters {
  kind?: SearchKind;
  calendar_ids?: string[];
  list_ids?: string[];
  /** ISO 8601 lower bound (date or datetime). */
  since?: string | null;
  /** ISO 8601 upper bound (date or datetime). */
  until?: string | null;
  /** Event-type filter — ignored when `kind = 'tasks'`. */
  event_type?: EventTypeFilter;
  /** Task-status whitelist — empty means no restriction. */
  task_statuses?: Task['status'][];
}

export const search = (query: string, filters?: SearchFilters) =>
  invoke<SearchResults>('search', { query, filters: filters ?? null });

// ── Reminders ──────────────────────────────────────────────────────────────

export interface UpcomingReminder {
  item_id: string;
  item_kind: 'event' | 'task';
  title: string;
  /** ISO 8601 UTC. */
  trigger_at: string;
}

export const listUpcomingReminders = () =>
  invoke<UpcomingReminder[]>('list_upcoming_reminders');

// ── Accounts ───────────────────────────────────────────────────────────────

export const listAccounts = () => invoke<Account[]>('list_accounts');

/** §19.11 step 8 — accounts whose keychain credentials are
 *  absent on this device. After `accept_remote_dataset` on a
 *  fresh device, the snapshot has populated the `accounts`
 *  table but the OS keychain is empty for every entry (secrets
 *  are device-local). The wizard reads this to drive the
 *  "Konten verbinden" prompt. Local account always excluded. */
export const listAccountsMissingCredentials = () =>
  invoke<Account[]>('list_accounts_missing_credentials');

/** Attach a password / API token to an existing account row
 *  pulled in via the snapshot. Used by the onboarding wizard
 *  for password-based backends (CalDAV, iCal-with-auth, EWS,
 *  Vikunja, Todoist). OAuth backends use their dedicated
 *  reconnect command instead. */
export const setAccountSecret = (accountId: string, secret: string) =>
  invoke<void>('set_account_secret', { accountId, secret });

/** Re-run the provider sign-in for an existing account. Opens the system
 *  browser. Works for every adapter that declares an OAuth block — the client,
 *  the tenant and anything else the account was configured with come off the
 *  row and its adapter's schema, host-side. Tokens land in the keychain under
 *  the existing account id, preserving downstream references. */
export const reconnectAccount = (accountId: string) =>
  invoke<void>('reconnect_account', { accountId });


export const deleteAccount = (id: string) =>
  invoke<void>('delete_account', { id });

/** Rename an account (change its user-visible `display_name`). The new
 *  name syncs to other devices via the event log. Returns the updated
 *  row. The local account can be renamed too. */
export const renameAccount = (id: string, newName: string) =>
  invoke<Account>('rename_account', { id, newName });

/** Adapter-specific CalDAV config JSON shape that lives in
 *  `accounts.config_json`. Mirrors the backend `CaldavAccountConfig`. */
export interface CaldavConfig {
  server_url: string;
  username: string;
  auth_kind: 'basic' | 'bearer';
}

/**
 * Round-trip the entered credentials without persisting anything, for ANY
 * adapter that declares an account schema. Same values the connect call takes,
 * split by the same schema on the Rust side — so a test and a connect cannot
 * disagree about what a field means.
 */
/**
 * Run one action the adapter declared on its connect form. Returns the values
 * the form should now carry, keyed by FIELD key — a field the action did not
 * produce is simply absent rather than blanked.
 */
export const runAccountAction = (
  adapter_kind: AdapterKind,
  action_key: string,
  values: Record<string, string | boolean>,
) =>
  invoke<Record<string, string>>('run_account_action', {
    request: { adapter_kind, action_key, values },
  });

export const testAccount = (
  adapter_kind: AdapterKind,
  values: Record<string, string | boolean>,
) => invoke<void>('test_account', { request: { adapter_kind, values } });

export const testCaldavConnection = (
  server_url: string,
  username: string,
  password: string,
) =>
  invoke<void>('test_caldav_connection', {
    request: { server_url, username, password },
  });

/** Public-feed config persisted as JSON in `accounts.config_json`.
 *  Mirrors the backend `IcalAccountConfig`. */
export interface IcalConfig {
  feed_url: string;
  username: string | null;
}

export const testIcalFeed = (
  feed_url: string,
  username: string | null,
  password: string | null,
) =>
  invoke<void>('test_ical_feed', {
    request: { feed_url, username, password },
  });

/** EWS config persisted as JSON in `accounts.config_json`. Mirrors
 *  the backend `EwsAccountConfig`. The endpoint is the user-supplied
 *  Exchange Web Services URL (e.g. `https://mail.example.org/EWS/Exchange.asmx`);
 *  autodiscover lands in a later phase. */
export interface EwsConfig {
  endpoint: string;
  username: string;
}

export const testEwsConnection = (
  endpoint: string,
  username: string,
  password: string,
) =>
  invoke<void>('test_ews_connection', {
    request: { endpoint, username, password },
  });

/** Vikunja config persisted as JSON in `accounts.config_json`. Mirrors
 *  the backend `VikunjaAccountConfig`. The server URL points at the
 *  Vikunja root (e.g. `https://try.vikunja.io`); the API token lives
 *  only in the keychain.  */
export interface VikunjaConfig {
  server_url: string;
}

/** Round-trip a single `GET /projects` against the Vikunja server
 *  with the provided API token. Catches typo'd URLs, unreachable
 *  servers and revoked tokens before persistence. */
export const testVikunjaConnection = (
  server_url: string,
  api_token: string,
) =>
  invoke<void>('test_vikunja_connection', {
    request: { server_url, api_token },
  });

/** Todoist is hosted — there's no server URL to configure. The
 *  account config is an empty JSON object (`{}`) by default; only
 *  the optional `account_label` shows up here if the dialog
 *  captured one. */
export interface TodoistConfig {
  account_label?: string | null;
}

/** Round-trip a `GET /projects` against api.todoist.com with the
 *  supplied Bearer token. Catches revoked tokens / unreachable API
 *  before persisting the account row. */
export const testTodoistConnection = (api_token: string) =>
  invoke<void>('test_todoist_connection', {
    request: { api_token },
  });

/** Result of an Autodiscover lookup. `account_email` may differ from
 *  what the user typed if a `<RedirectAddr>` step rewrote the
 *  identity along the way — the dialog can use it to refill the
 *  username field. */
export interface DiscoveredEndpoints {
  ews_url: string;
  account_email: string;
}

/** Walk Microsoft's POX-Autodiscover URL cascade for the user's
 *  domain to find the matching EWS endpoint. Used by the
 *  AccountsDialog "Discover" button so users don't have to guess the
 *  URL themselves. On failure the dialog falls back to the existing
 *  manual-entry flow. */
export const discoverEwsEndpoint = (email: string, password: string) =>
  invoke<DiscoveredEndpoints>('discover_ews_endpoint', {
    request: { email, password },
  });

/** Kick off the Google OAuth dance. The backend opens the system
 *  browser to Google's consent screen and blocks until the user
 *  completes the flow (or hits the 5-min timeout). On success the
 *  refresh + access tokens land in the keychain and the new account
 *  is registered with the adapter registry — the resolved
 *  `Account` row is returned just like a `createAccount` call.
 *
 *  `client_id` and `client_secret` are the matching pair from the
 *  user's Desktop-app OAuth client in their Google Cloud Console.
 *  Google requires both on the token endpoint even though PKCE is
 *  in use — their own docs concede the "secret" isn't actually a
 *  secret for installed apps. See AccountsDialog help text for the
 *  setup walkthrough. */
export const connectGoogleAccount = (
  client_id: string,
  client_secret: string,
  display_name: string,
) =>
  invoke<Account>('connect_google_account', {
    request: { client_id, client_secret, display_name },
  });

/** Kick off the Microsoft OAuth dance. Same shape as Google, but:
 *  - No client_secret (Microsoft honours PKCE-public-client semantics).
 *  - `authority` selects which Microsoft accounts may sign in:
 *    `common` (any), `consumers` (personal MS accounts only),
 *    `organizations` (work/school only), or a specific tenant
 *    GUID. Defaults to `common` when omitted.
 *  - The Azure portal app registration must have the same
 *    "supported account types" setting as the authority you pass. */
export const connectMicrosoftAccount = (
  client_id: string,
  display_name: string,
  authority?: string,
) =>
  invoke<Account>('connect_microsoft_account', {
    request: { client_id, display_name, authority: authority ?? null },
  });

/** The wire shapes for a schema-driven connect form live in `@aperio/shared`:
 *  the mobile app renders the same declaration from the same types. */
export type {
  AccountFormField,
  AccountFormOauth,
  AccountFormSpec,
} from '@aperio/shared';
import type {
  AccountFormSpec,
  EventGroup,
  SuggestionDecline,
} from '@aperio/shared';

/** One adapter this build can connect an account for. */
export interface AdapterKindInfo {
  kind: AdapterKind;
  /** Whether a NEW account of this kind may be offered. False for a kind its
   *  plugin only ADOPTED from an adapter it replaced: the accounts that carry
   *  it stay listed, groupable and repairable, but there is nothing left to
   *  create another one with.
   *
   *  Filter on it wherever a surface CREATES something; ignore it wherever a
   *  surface DESCRIBES what already exists. */
  offered: boolean;
  /** Whether an account of this kind ALREADY exists and cannot be created or
   *  deleted — the built-in store, and nothing else today.
   *
   *  The companion to {@link offered}, because the two answer different
   *  questions. A surface that CREATES filters on `offered`; a surface that
   *  offers a CHOICE among things that can already exist accepts
   *  `offered || implicit`. Without the distinction the built-in store — the
   *  one storage backend needing no account created first — would drop out of
   *  the sync form, and "a folder on this device" would stop being an answer. */
  implicit: boolean;
  /** The plugin's own display name — the label to use when the app has no
   *  translation for this kind, which is the normal case for a third-party
   *  plugin. */
  name: string;
  plugin_id: string;
  /** Whether accounts of this adapter own calendars and task lists. */
  owns_containers: boolean;
  /** Whether an account of this adapter is worth having on its own — it holds
   *  calendars, task lists, contacts or meetings. False for an adapter whose
   *  only capability is `sync`. The Add-account picker's question. */
  holds_data: boolean;
  /** Whether this adapter can be chosen as the sync target. The sync settings'
   *  question. Not exclusive with the one above. */
  can_sync: boolean;
  /** Whether it connects through the generic schema-driven flow. False for the
   *  adapters still on the older per-kind path. */
  declares_account_schema: boolean;
  /** Whether connecting it means a provider sign-in rather than a typed
   *  credential. Lets a list of accounts decide how to offer a repair
   *  without fetching each adapter's form spec. */
  declares_oauth: boolean;
}

/** Every adapter this build can connect an account for.
 *
 *  Asked of the host rather than written into the UI: which adapters exist is
 *  decided by which plugins are installed, and that can change without the
 *  frontend being rebuilt. Host-internal kinds (the local store, the device
 *  calendar) are not included — each frontend adds those itself where they
 *  make sense. */
export const listAdapterKinds = () =>
  invoke<AdapterKindInfo[]>('list_adapter_kinds');

/** The connect form an adapter declares, or `null` when it declares none —
 *  which is the correct answer for the adapters still on the older per-kind
 *  connect path, and for plugins that have no accounts at all. */
export const accountFormSpec = (adapter_kind: AdapterKind, lang?: string) =>
  invoke<AccountFormSpec | null>('account_form_spec', {
    adapterKind: adapter_kind,
    lang,
  });

/** Create an account for any adapter that declares a schema.
 *
 *  `values` is keyed by the schema's field keys. The backend runs the OAuth
 *  sign-in if the schema has one, keeps every declared secret in the platform
 *  keychain, and puts only the non-secret half in the account row. */
export const connectAccount = (request: {
  adapter_kind: AdapterKind;
  display_name: string;
  values: Record<string, string | boolean>;
}) => invoke<Account>('connect_account', { request });

/** A meeting on the provider side. */
export interface Meeting {
  id: string;
  join_url: string;
  title: string;
  start_time: string | null;
  end_time: string | null;
  password: string | null;
}

/** The record that a meeting was created BY Aperio for a given event.
 *
 *  Host-local: it says which provider-side meeting this device can still take
 *  down. An event carrying someone else's meeting link has no binding, which is
 *  why the editor offers Join but not Remove for it. */
export interface EventMeetingBinding {
  event_id: string;
  account_id: string;
  meeting_id: string;
  join_url: string;
  created_at: string;
}

/** Create a meeting for an event, write its link into the event, and record the
 *  binding — one call, because doing the three separately is how an event ends
 *  up linking a meeting nobody can delete. Returns the event as saved. */
export const attachMeeting = (request: {
  event_id: string;
  calendar_id: string;
  account_id: string;
  /** Link the account's permanent room instead of minting a meeting. */
  use_personal_room?: boolean;
  /**
   * Which language the join block is written in. Frozen into the event, so it
   * is decided per meeting rather than followed from the UI.
   */
  invitation_lang?: string;
}) => invoke<{ event: CalendarEvent; meeting: Meeting }>('attach_meeting', { request });

/** Delete the meeting Aperio created for an event and take its link back out.
 *  `null` when the event had no meeting of ours. */
export const detachMeeting = (request: {
  event_id: string;
  calendar_id: string;
}) => invoke<CalendarEvent | null>('detach_meeting', { request });

/** The meeting Aperio created for this event — or for any copy of it.
 *
 *  Pass the calendar so a group can answer: a meeting hangs on exactly one
 *  event, and which one is a coincidence of the moment it was attached. */
export const eventMeeting = (event_id: string, calendar_id?: string) =>
  invoke<EventMeetingBinding | null>('event_meeting', {
    eventId: event_id,
    calendarId: calendar_id,
  });

// ── Event groups (which events mean the same appointment) ──
//
// Nothing here reaches a provider: grouping two events changes neither of
// them, and ungrouping leaves both exactly as they were.

/** A member on the way in: the reference plus its signature at this moment. */
export interface NewGroupMember {
  calendar_id: string;
  /** Series master id. */
  event_id: string;
  title: string;
  starts_at: string;
}

/** Declare that these events mean the same appointment.
 *
 *  Joins an existing group when exactly one of them is already in one — the
 *  natural "and this one too". Rejects with `event_group_conflict` when two of
 *  them are in DIFFERENT groups, because merging two claims about what an
 *  appointment is cannot be inferred from a request that did not ask for it. */
export const groupEvents = (members: NewGroupMember[]) =>
  invoke<EventGroup>('group_events', { members });

/** Take one event out of its group.
 *
 *  `null` when that dissolved the group (fewer than two members left) or the
 *  event was not grouped at all. */
export const ungroupEvent = (calendar_id: string, event_id: string) =>
  invoke<EventGroup | null>('ungroup_event', {
    calendarId: calendar_id,
    eventId: event_id,
  });

/** Dissolve a whole group. The events themselves are untouched. */
export const dissolveEventGroup = (group_id: string) =>
  invoke<void>('dissolve_event_group', { groupId: group_id });

/** Record that two events are NOT the same appointment.
 *
 *  Silences the OFFER only — grouping them by hand still works and never
 *  consults this. */
export const declineGroupSuggestion = (
  first: { calendar_id: string; event_id: string },
  second: { calendar_id: string; event_id: string },
) => invoke<void>('decline_group_suggestion', { first, second });

/** Every pair the user has said is not one appointment. */
export const groupSuggestionDeclines = () =>
  invoke<SuggestionDecline[]>('group_suggestion_declines');

/** Point one member at the id its event carries now.
 *
 *  A repair of Aperio's own bookkeeping — the same events mean the same
 *  appointment before and after — so it is applied silently by whichever view
 *  noticed, never announced as a change the user made. */
export const healEventGroupMember = (payload: {
  group_id: string;
  calendar_id: string;
  old_event_id: string;
  new_event_id: string;
}) =>
  invoke<void>('heal_event_group_member', {
    groupId: payload.group_id,
    calendarId: payload.calendar_id,
    oldEventId: payload.old_event_id,
    newEventId: payload.new_event_id,
  });

/** Every group any of these events belongs to — whole, including members
 *  outside the range asked about. */
export const eventGroupsForEvents = (
  events: { calendar_id: string; event_id: string }[],
) => invoke<EventGroup[]>('event_groups_for_events', { events });

/** One person the provider lists on a meeting. */
export interface MeetingInvitee {
  email: string;
  display_name: string | null;
  co_host: boolean;
}

/** Everything known about an event's meeting, in one answer. */
export interface EventMeetingInspection {
  /** Set when Aperio created it and can therefore remove it. */
  binding: EventMeetingBinding | null;
  /** The meeting as the provider currently describes it, including who IT says
   *  is invited — which is often not what the calendar event says. */
  meeting: (Meeting & { invitees?: MeetingInvitee[] }) | null;
  /** The account that recognised the meeting; needed to adopt it. */
  account_id: string | null;
}

/** Ask about the meeting on an event: is it ours, what does the provider say
 *  about it, and who is really invited. Looks it up by the join link, so it
 *  answers for meetings Aperio did not create. */
export const inspectEventMeeting = (request: {
  event_id: string;
  calendar_id: string;
}) => invoke<EventMeetingInspection>('inspect_event_meeting', { request });

/** Take responsibility for a meeting Aperio did not create, so it can also be
 *  removed. Writes nothing to the event — the link is already there. */
export const adoptMeeting = (request: {
  event_id: string;
  calendar_id: string;
  account_id: string;
  meeting_id: string;
  join_url: string;
}) => invoke<EventMeetingBinding>('adopt_meeting', { request });

/** Which container namespace an override applies to. Calendars and
 *  task lists have disjoint ids today but the backend keeps them
 *  separately namespaced so a future code-path can enforce kind. */
export type ContainerKind = 'calendar' | 'task_list' | 'contact_list';

/** Persist a local rename override for a calendar / task list. The
 *  rename never reaches the source server — read-time projection
 *  only. Power-user escape hatch; the canonical entry point for
 *  rename is `renameContainer` below. */
export const setContainerNameOverride = (
  container_id: string,
  kind: ContainerKind,
  name: string,
) =>
  invoke<void>('set_container_name_override', {
    containerId: container_id,
    kind,
    name,
  });

/** Drop the override and revert to the source name on the next read. */
export const clearContainerNameOverride = (
  container_id: string,
  kind: ContainerKind,
) =>
  invoke<void>('clear_container_name_override', {
    containerId: container_id,
    kind,
  });

/** Bind (or, with `color_label_id = null`, unbind) a container's color to
 *  a color-label (DESIGN §6.5/§8.2). Local calendars/lists store it on
 *  the synced row; external containers via a host-local override. */
export const setContainerColorLabel = (
  container_id: string,
  kind: ContainerKind,
  color_label_id: string | null,
) =>
  invoke<void>('set_container_color_label', {
    containerId: container_id,
    kind,
    colorLabelId: color_label_id,
  });

/** Set / clear a section's color label. Routes host-side: local sections
 *  store the binding on their (synced) row; external sections (Todoist /
 *  Vikunja) store a local color override. `list_id` routes the call. */
export const setSectionColor = (
  section_id: string,
  list_id: string,
  color_label_id: string | null,
) =>
  invoke<void>('set_section_color', {
    sectionId: section_id,
    listId: list_id,
    colorLabelId: color_label_id,
  });

/** Set / clear the host-local color override for an EXTERNAL event whose
 *  calendar can't store a per-event color (iCloud etc.). No-op for local
 *  events (those keep their color on the row via `update_event`). The
 *  `event_id` is the series master id. */
export const setEventColor = (
  event_id: string,
  calendar_id: string,
  color_label_id: string | null,
) =>
  invoke<void>('set_event_color', {
    eventId: event_id,
    calendarId: calendar_id,
    colorLabelId: color_label_id,
  });

/** Outcome of [`renameContainer`]. `synced_to_source = false` means
 *  the adapter declared the operation unsupported (read-only source
 *  like an iCal feed) and the rename was saved as a local override
 *  instead. The frontend can use this to nudge the user. */
export interface RenameOutcome {
  synced_to_source: boolean;
}

/** Canonical rename entry point. The backend tries to push the new
 *  name to the source server (CalDAV PROPPATCH, local SQLite UPDATE,
 *  future Google PATCH) and falls back to a local override only when
 *  the adapter doesn't support remote rename. */
export const renameContainer = (
  container_id: string,
  kind: ContainerKind,
  name: string,
) =>
  invoke<RenameOutcome>('rename_container', {
    containerId: container_id,
    kind,
    name,
  });

// ── User preferences (key/value) ─────────────────────────────────────────

/** Read a stored user-pref value. Returns `null` when the key has
 *  never been set — the caller picks a default. Values are opaque
 *  strings; serialise JSON when you need structure. */
export const getUserPref = (key: string) =>
  invoke<string | null>('get_user_pref', { key });

/** Upsert a user-pref value. */
export const setUserPref = (key: string, value: string) =>
  invoke<void>('set_user_pref', { key, value });

/** Drop the stored value (no-op when nothing was set). */
export const deleteUserPref = (key: string) =>
  invoke<void>('delete_user_pref', { key });

// ── Autostart (launch at OS login, §17.4) ─────────────────────────────

/** Whether Aperio is registered to launch at OS login. The OS registration is
 *  the source of truth; the General settings toggle reflects this value. */
export const autostartIsEnabled = () =>
  invoke<boolean>('autostart_is_enabled');

/** Enable or disable launch-at-login. */
export const autostartSet = (enabled: boolean) =>
  invoke<void>('autostart_set', { enabled });

// ── System tray (close/minimize to tray) ──────────────────────────────

/** Whether the platform/desktop exposes a system tray. The General
 *  settings panel uses this to enable/disable the tray toggles. */
export const trayAvailable = () => invoke<boolean>('tray_available');

/** Close the window — hides to the tray when `window.closeToTray` is on
 *  and a tray exists, otherwise quits. Called by the title-bar X. */
export const requestWindowClose = () =>
  invoke<void>('request_window_close');

/** Minimize the window — hides to the tray when `window.minimizeToTray` is
 *  on and a tray exists, otherwise minimizes normally. Called by the
 *  title-bar minimize button. */
export const requestWindowMinimize = () =>
  invoke<void>('request_window_minimize');

/** Push localized labels onto the tray menu items. The frontend calls this
 *  once i18n is ready and on every language change so the tray menu follows
 *  the app language. No-op on platforms without a tray. */
export const setTrayLabels = (show: string, quit: string) =>
  invoke<void>('set_tray_labels', { show, quit });

// ── Custom notification sounds (§14.4) ────────────────────────────────────

/** A custom sound stored on disk, content-addressed by SHA-256. Only
 *  the hash is persisted in a `SoundConfig`; `ext` is informational for
 *  the picker's list. */
export interface ImportedSound {
  sha256: string;
  ext: string;
}

/** Import a user-chosen audio file into the content-addressed store and
 *  return its hash + extension. Enforces the ≤5 MB / allowed-format
 *  limits backend-side. */
export const importSound = (path: string) =>
  invoke<ImportedSound>('import_sound', { path });

/** List every custom sound currently in the store. */
export const listCustomSounds = () =>
  invoke<ImportedSound[]>('list_custom_sounds');

/** Play a sound once (the SoundPicker's "Test" button). Only `custom`
 *  sources produce audio; system/silent are no-ops. */
export const previewSound = (config: SoundConfig) =>
  invoke<void>('preview_sound', { config });

/** Delete a custom sound from the store by hash (idempotent). */
export const deleteCustomSound = (sha256: string) =>
  invoke<void>('delete_custom_sound', { sha256 });

/**
 * Wake the reminder scheduler so it re-scans on the next tick AND
 * drop its external-trigger cache so the next scan re-fans out to
 * every adapter. The frontend calls this after editing things the
 * scheduler reads but doesn't watch directly — most notably the
 * per-calendar "Standard-Hinweise" in Settings → Kalender. Without
 * it, a freshly added default reminder doesn't reach the firing
 * loop until the cache TTL (~5 min) expires.
 *
 * Cheap on the wire: no payload, the scheduler's `Notify` coalesces
 * repeat calls, so it's safe to invoke per-change rather than
 * trying to batch.
 */
export const invalidateReminders = () =>
  invoke<void>('invalidate_reminders');

/**
 * Push the set of HIDDEN (sidebar-unchecked) calendar ids to the reminder
 * scheduler so event reminders on those calendars are suppressed — hiding a
 * calendar silences its reminders too. The visibility lives in localStorage
 * (frontend-only), so the Rust scheduler can't read it; this bridges it. Called
 * on startup + whenever the sidebar selection changes.
 */
export const setReminderHiddenCalendars = (hiddenCalendarIds: string[]) =>
  invoke<void>('set_reminder_hidden_calendars', { hiddenCalendarIds });

// ── Native context menu ──────────────────────────────────────────────────

/** One entry in the native context menu. The shape supports four
 *  kinds:
 *
 *    - `text` (default): a plain action row. `id` round-trips through
 *      the OS menu API and identifies what the user picked.
 *    - `check`: a row whose check-mark state is driven by `checked`.
 *      Win32 / NSMenu / GTK draw their own glyph.
 *    - `submenu`: a nested menu with its own `items` array. `label`
 *      is the visible parent text; `id` is unused (the submenu
 *      header itself is never "selected").
 *    - `separator`: a horizontal divider — no id, no label, never
 *      selected.
 *
 *  Backward compat: omitting `kind` is treated as `text`. */
export type ContextMenuItemRequest =
  | { kind?: 'text'; id: string; label: string }
  | { kind: 'check'; id: string; label: string; checked: boolean }
  | { kind: 'submenu'; label: string; items: ContextMenuItemRequest[] }
  | { kind: 'separator' };

/** Show a native OS context menu (Win32 / NSMenu / GTK) anchored
 *  either at the cursor (omit `position`) or at the given
 *  window-logical coordinates. Returns the chosen item's id, or
 *  `null` when the user dismissed without selecting.
 *
 *  Frontend keyboard triggers should pass a position so the menu
 *  appears near the focused row rather than at a stale cursor
 *  location; right-click triggers should omit it and let the OS
 *  pick the cursor. */
export const showContextMenu = (
  items: ContextMenuItemRequest[],
  position?: { x: number; y: number },
) =>
  invoke<string | null>('show_context_menu', {
    request: { items, position: position ?? null },
  });

// ── Contacts (DESIGN.md §10) ────────────────────────────────────────────

export const listContactLists = () =>
  invoke<ContactList[]>('list_contact_lists');

export interface CreateContactListRequest {
  name: string;
  /** Bind the new address book's color to this color-label id (or null). */
  color_label?: string | null;
}

export const createContactList = (request: CreateContactListRequest) =>
  invoke<ContactList>('create_contact_list', { request });

export const deleteContactList = (id: string) =>
  invoke<void>('delete_contact_list', { id });

export const renameContactList = (id: string, newName: string) =>
  invoke<void>('rename_contact_list', { id, newName });

export const getContacts = (listId: string) =>
  invoke<Contact[]>('get_contacts', { listId });

/** Cross-account contacts search. Hits up to 50 rows per source;
 *  the local adapter caps internally, external adapters do
 *  whatever they do. Empty / whitespace queries return an empty
 *  array without a round-trip cost on the local side. */
export const searchContacts = (query: string) =>
  invoke<Contact[]>('search_contacts', { query });

export interface CreateContactRequest extends NewContact {
  list_id: string;
}

export const createContact = (request: CreateContactRequest) =>
  invoke<Contact>('create_contact', { request });

export const updateContact = (contact: Contact) =>
  invoke<Contact>('update_contact', { contact });

/** Delete a contact. `listId` is an optional routing hint — the
 *  backend uses it to find the owning account without walking
 *  every contact list. Frontend callers always know the list
 *  (they just rendered the row), so always pass it. */
export const deleteContact = (id: string, listId?: string) =>
  invoke<void>('delete_contact', {
    id,
    listId: listId ?? null,
  });

/** Pull the avatar bytes for a contact. Returns `null` when the
 *  contact has no photo — the dialog renders the initials
 *  placeholder in that case. */
export const getContactPhoto = (id: string, listId?: string) =>
  invoke<ContactPhoto | null>('get_contact_photo', {
    id,
    listId: listId ?? null,
  });

/** Upload (or replace) the contact's avatar. The bytes travel
 *  base64-encoded inside `photo.data`; on the wire the Rust side
 *  custom-serdes them into `Vec<u8>`. */
export const setContactPhoto = (
  id: string,
  photo: ContactPhoto,
  listId?: string,
) =>
  invoke<void>('set_contact_photo', {
    id,
    listId: listId ?? null,
    photo,
  });

/** Clear the avatar without touching any other field. Idempotent
 *  — calling it on a contact without a photo succeeds silently. */
export const deleteContactPhoto = (id: string, listId?: string) =>
  invoke<void>('delete_contact_photo', {
    id,
    listId: listId ?? null,
  });

// ── Contact sync scheduler (Phase 10j, DESIGN.md §10.5) ────────────────

/** Snapshot of the backend's contact sync scheduler. `lastSyncedAt`
 *  is RFC 3339 or `null` if no pass has completed yet; the frontend
 *  formats it via the user's locale for the panel footer. */
export interface ContactsSyncStatus {
  last_synced_at: string | null;
  interval_minutes: number;
  in_flight: boolean;
  /** Current value of the `contacts.includeReadOnlyOnSync` pref.
   *  The Settings → Kontakte checkbox seeds itself from this so
   *  the toggle survives restarts and applies to both manual and
   *  periodic sync passes. */
  include_read_only_on_sync: boolean;
}

/** Payload of the `contacts-synced` Tauri event the backend emits
 *  after every sync pass completes — manual, periodic, or
 *  app-start. The frontend uses `lastSyncedAt` to update the
 *  footer and `succeededAccounts` / `failedAccounts` to decide
 *  which lists to refetch. */
export interface ContactsSyncedPayload {
  last_synced_at: string;
  succeeded_accounts: string[];
  failed_accounts: string[];
}

/** Trigger a manual sync pass. `includeReadOnly` is an explicit
 *  per-call override: `true` always pulls the heavy read-only
 *  sentinel lists (GAL, Suggested People, Other Contacts,
 *  Workspace Directory), `false` always skips them, `undefined`
 *  (the default) reads the user's persisted
 *  `contacts.includeReadOnlyOnSync` pref so manual and periodic
 *  syncs share the same behaviour. */
export const syncContactsNow = (includeReadOnly?: boolean) =>
  invoke<boolean>('sync_contacts_now', {
    includeReadOnly: includeReadOnly ?? null,
  });

/** Persist the "also pull read-only directories" toggle from
 *  Settings → Kontakte. Backend writes
 *  `user_prefs.contacts.includeReadOnlyOnSync`; the scheduler
 *  re-reads on every tick so the new value applies on the next
 *  pass. */
export const setContactsIncludeReadOnlyOnSync = (enabled: boolean) =>
  invoke<void>('set_contacts_include_read_only_on_sync', { enabled });

export const getContactsSyncStatus = () =>
  invoke<ContactsSyncStatus>('get_contacts_sync_status');

/** Wipe every external adapter's in-memory contact cache and reset
 *  the persisted "last synced" timestamp to never. Backs the
 *  "Cache leeren" button in Settings → Kontakte (DESIGN.md §10.6).
 *  Returns the number of adapters whose invalidate succeeded;
 *  failed ones log on the backend but don't sink the call. Local
 *  contact rows are user data, not a cache — this never touches
 *  the SQLite `contacts` table. */
export const clearContactsCache = () =>
  invoke<number>('clear_contacts_cache');

/** Update the periodic-sync interval (in minutes). The backend
 *  clamps to [1, 1440] and returns the value actually persisted,
 *  so a typo gets corrected rather than crashing the scheduler. */
export const setContactsSyncInterval = (minutes: number) =>
  invoke<number>('set_contacts_sync_interval', { minutes });

// ── External snapshot cache (CACHE-3) ─────────────────────────────────

/** Status of the background external-cache refresher. `refreshing`
 *  is true while a warm pass runs; `last_refreshed_at` is RFC 3339
 *  (or null before the first pass) and survives restarts. */
export interface CacheRefreshStatus {
  refreshing: boolean;
  last_refreshed_at: string | null;
  /** Containers the running pass will refresh (null outside a pass / before
   *  enumeration) — drives "fetched X of N" external-refresh progress. */
  total_targets: number | null;
  /** Containers refreshed so far in the running pass (null outside a pass). */
  fetched_targets: number | null;
}

/** Payload of the `cache-refresh-status` Tauri event, emitted at the
 *  start and end of every warm pass. Same shape as the status query. */
export type CacheRefreshStatusPayload = CacheRefreshStatus;

/** Kick an immediate background warm pass (manual "refresh now").
 *  Returns as soon as the pass is queued. */
export const refreshExternalCache = () =>
  invoke<void>('refresh_external_cache');

/** Current refresher status — seeds the toolbar indicator on mount. */
export const getCacheRefreshStatus = () =>
  invoke<CacheRefreshStatus>('get_cache_refresh_status');

/** Every account's currently-failing containers (the silent-staleness
 *  warning surface). Empty when all refreshes are healthy. */
export const getRefreshErrors = () =>
  invoke<AccountRefreshErrors[]>('get_refresh_errors');

/** Force a FULL cold re-sync of one external account: clear its delta tokens +
 *  cached window across every container, then kick a warm pass so each
 *  re-bootstraps from the provider. Cached rows stay as an offline fallback
 *  until replaced; credentials are untouched. The recovery action for a "stuck"
 *  external cache (a bootstrap that cached an incomplete set as complete). */
export const resetAccountSync = (accountId: string) =>
  invoke<void>('reset_account_sync', { accountId });

// ── Cross-device sync (Phase Sd–Si, DESIGN.md §19) ───────────────────

/** Adapter family the user picked. `none` is the explicit
 *  disconnect — sync stops, no further pushes, but already-pushed
 *  data on the remote stays where it is. */
export type SyncAdapterKind = 'local' | 'webdav' | 'sftp' | 'none';

/** Adapter-family-specific config. Discriminated union keyed by
 *  `kind` — matches the Rust `SyncAdapterConfig` enum on the wire.
 *
 *  WebDAV / SFTP `password` is optional: omitting it on a re-edit
 *  (URL or user only) keeps the previously-stored keychain
 *  password in place. Supplying an empty string is treated the
 *  same way. */
export type SyncAdapterConfig =
  | { kind: 'local'; path: string }
  | {
      kind: 'webdav';
      url: string;
      user: string;
      password?: string | null;
    }
  | {
      kind: 'sftp';
      host: string;
      port: number;
      user: string;
      path: string;
      /** `"password"` or `"key"`. The two halves of the union
       *  share the host/port/user/path payload, so we model it
       *  as a single shape with a string discriminator rather
       *  than two TypeScript variants — matches the Rust enum's
       *  serialised form. */
      auth_method: 'password' | 'key';
      /** Used when `auth_method = "password"`. Empty / null
       *  reuses the keychain entry. */
      password?: string | null;
      /** Used when `auth_method = "key"`. Absolute path to the
       *  PEM / OpenSSH-format private key file. */
      key_path?: string | null;
      /** Optional passphrase for an encrypted key. Empty / null
       *  reuses the keychain entry (or is treated as "no
       *  passphrase" on first-run). */
      key_passphrase?: string | null;
    }
  | {
      kind: 'dropbox';
      /** Dropbox app's OAuth client_id from
       *  dropbox.com/developers/apps. Not a secret per the
       *  Dropbox docs — it identifies the app, not the user. */
      client_id: string;
      /** Optional. Public (PKCE-only) Dropbox apps leave this
       *  empty; confidential apps pass the secret as
       *  documented by the developer console. */
      client_secret?: string;
      /** Remote folder under the user's Dropbox, e.g.
       *  `/aperio`. Empty string addresses the app's root. */
      path?: string;
    }
  | {
      kind: 'googledrive';
      /** Google OAuth client_id from console.cloud.google.com.
       *  Required. */
      client_id: string;
      /** Google OAuth client_secret. Required — Google's
       *  installed-app flow always exchanges the secret in
       *  the token endpoint call, even though their docs
       *  note it isn't strictly secret in this context. */
      client_secret: string;
      /** Human-readable folder name under My Drive that
       *  holds the sync dataset. Empty string lets the
       *  adapter default it to "Aperio". */
      folder_name?: string;
    }
  | {
      kind: 'ftp';
      host: string;
      port: number;
      user: string;
      path: string;
      /** `"explicit"` (AUTH TLS upgrade, default port 21),
       *  `"implicit"` (TLS-first handshake, default port 990),
       *  or `"plain"` (no TLS, port 21). Plain is an opt-in
       *  for legacy LAN scenarios — the frontend gates it
       *  behind a visible warning. */
      mode: 'explicit' | 'implicit' | 'plain';
      /** Empty / null reuses the keychain entry — same contract
       *  as the WebDAV / SFTP fields. */
      password?: string | null;
    }
  | { kind: 'none' };

/** Read-only status snapshot. The state indicator polls this on a
 *  short cadence + refreshes on `sync-status` events. */
export interface SyncStatus {
  configured: boolean;
  in_flight: boolean;
  /** RFC 3339 timestamp of the last successful round, or `null`
   *  when no round has completed yet. The status bar formats it
   *  via the user's locale. */
  last_synced_at: string | null;
  /** Currently-configured periodic interval, minutes. Default 5
   *  (per §19.8); the Settings → Synchronisation slider edits it
   *  via `setSyncInterval`. */
  interval_minutes: number;
  /** Phase Sk — whether the current dataset is end-to-end
   *  encrypted. Surfaced so the Settings panel can render a
   *  "🔒 verschlüsselt" badge without a separate fetch. */
  e2e_enabled: boolean;
  /** Phase Sl — latched `true` when the last sync round failed
   *  with `SchemaTooOld`. Triggers the non-dismissible §19.13
   *  update modal. Cleared on the next successful round. */
  schema_too_old: boolean;
  /** When `schema_too_old`, the minimum Aperio version the
   *  dataset requires. Shown verbatim in the update prompt so
   *  the user knows which version to install. */
  min_app_version_required: string | null;
  /** `true` when the scheduler has seen three or more
   *  consecutive failed rounds. Drives a warning tone on the
   *  status indicator + a banner in the Settings panel so the
   *  user doesn't have to read the log to notice a remote
   *  that's been unreachable for a while. Cleared on the next
   *  successful round. */
  sustained_failure: boolean;
  /** §19.10: when set, this device is "stale" — the compactor
   *  has GCed log files we'd need for incremental catch-up. The
   *  resume dialog opens off this latch; the value is the RFC3339
   *  timestamp of the snapshot the user is being asked to re-pull
   *  (shown in the dialog body so they know which point in time
   *  the local data will jump to). Cleared after a successful
   *  `resumeStaleDevice` call. */
  stale_device_since: string | null;
  /** Stable identifier of the most recent sync-round failure
   *  ([`SyncError::code`] strings: "auth", "network", "io",
   *  "protocol", "not_found", "encryption_required",
   *  "schema_too_old", "stale_device", "internal"). Latched on
   *  failure, cleared on the next success. Lets the SyncStatusBar
   *  branch on the error kind — most notably to render an
   *  "auth failed, reconnect here" banner specifically for
   *  `"auth"` rather than the generic "Verbindungsfehler". */
  last_error_code: string | null;
}

/** Counters from one `syncNow` invocation. Surfaced so the
 *  Settings → Synchronisation panel can show "12 events applied"
 *  without a follow-up status fetch. */
export interface SyncRoundReport {
  pushed_logs: number;
  fetched_logs: number;
  applied: number;
  skipped_own: number;
  skipped_already_applied: number;
  skipped_unsupported: number;
  apply_failures: number;
  push_failures: number;
  fetch_failures: number;
  /** Field-level conflicts the applier recorded during this
   *  round (DESIGN.md §19.3). When > 0, the frontend fires a
   *  system notification and refreshes the unresolved-conflict
   *  count so the status badge updates. */
  conflicts: number;
}

/** Payload of the `sync-status` Tauri event the backend emits
 *  before + after every sync round. `report` is set on the
 *  post-round emit; `error` carries the orchestrator-level message
 *  when a round failed at the adapter layer. */
export interface SyncStatusPayload extends SyncStatus {
  report?: SyncRoundReport;
  error?: string;
}

/** Outcome of a compaction round (manual or auto). Surfaced in
 *  Settings → Synchronisation. */
export interface CompactionReport {
  snapshot_timestamp: string | null;
  deleted_logs: number;
  failed_deletes: number;
  stale_devices: number;
  snapshot_rows: number;
  snapshot_settings: number;
}

/** Onboarding preview result — what the §19.11 dialog needs to
 *  render before the user picks "übernehmen" vs "neu beginnen". */
export type SyncPreview =
  | { kind: 'empty' }
  | {
      kind: 'existing';
      schema_version: number;
      min_app_version: string;
      snapshot_timestamp: string | null;
      e2e_enabled: boolean;
      devices: SyncDeviceSummary[];
      /** Phase Sl — version compatibility verdict. The accept
       *  buttons gate on this; `app_too_old` pops the update
       *  modal instead of letting the user proceed. */
      compatibility: SyncCompatibility;
    };

/** Tagged result of `sync_core::check_compatibility`. Mirrors the
 *  Rust enum on the wire. */
export type SyncCompatibility =
  | { kind: 'ok' }
  | { kind: 'app_too_old'; required: string; running: string }
  | { kind: 'schema_ahead'; remote: number; local: number };

export interface SyncDeviceSummary {
  id: string;
  name: string | null;
  /** The device's CONTENT horizon — the newest log it holds. NOT when it was
   *  last here: on a quiet dataset this does not move however often the device
   *  syncs. Use {@link last_seen} for that. */
  last_seen_log: string;
  /** When the device last completed a round, or `null` on a dataset whose
   *  registry predates the field. `null` reads as "unknown", never as a very
   *  old date — the difference decides whether a row looks abandoned. */
  last_seen: string | null;
  app_version: string;
  stale: boolean;
  /** `true` when this entry refers to the current device — the
   *  dialog highlights it ("Dieses Gerät"). */
  is_this_device: boolean;
}

/** What this device calls itself, plus what it would suggest calling itself.
 *
 *  `suggested` is the machine's host name, offered on a device that has never
 *  been named; `null` when the host name cannot be read, which is not an error
 *  — the name is optional and the field simply starts empty. */
export interface DeviceNameInfo {
  configured: string | null;
  suggested: string | null;
}

export const syncDeviceName = () => invoke<DeviceNameInfo>('sync_device_name');

/** Rename this device; a blank name clears it. Reaches the other devices on
 *  the next sync round — the heartbeat notices the difference and pushes. */
export const setSyncDeviceName = (name: string) =>
  invoke<void>('set_sync_device_name', { name });

/** Every device registered on the dataset this one syncs through. Rejects with
 *  `not_configured` when this device syncs nowhere. */
export const listSyncDevices = () =>
  invoke<SyncDeviceSummary[]>('list_sync_devices');

/** Drop a device's registry entry.
 *
 *  Not a revocation: it removes the claim that the device is still
 *  participating, and one that still runs re-registers on its next round. It
 *  leaves the device's log files alone — the compactor collects those once the
 *  snapshot covers them. Refuses this device's own id. */
export const forgetSyncDevice = (deviceId: string) =>
  invoke<void>('forget_sync_device', { deviceId });

/** Counters returned by accept/adopt onboarding. */
export interface OnboardingReport {
  fetched_logs: number;
  applied: number;
  skipped_own: number;
  skipped_already_applied: number;
  skipped_unsupported: number;
  apply_failures: number;
  remote_was_empty: boolean;
  device_count: number;
}

/** One conflict row from `sync_conflicts`. The dialog renders
 *  `local_value` + `remote_value` as JSON-encoded scalars (string,
 *  number, ...); the frontend decodes them via `JSON.parse` for
 *  display. */
export interface SyncConflict {
  id: number;
  detected_at: string;
  row_kind: 'event' | 'task' | 'task_list' | 'calendar' | 'color_label';
  row_id: string;
  field: string;
  local_value: string | null;
  remote_value: string | null;
  remote_device_id: string;
  remote_timestamp: string;
  resolved: boolean;
  resolution: SyncResolutionChoice | null;
  resolved_at: string | null;
}

export type SyncResolutionChoice =
  | 'keep_local'
  | 'take_remote'
  | 'save_both';

// ── Steady-state sync commands ──

export const getSyncStatus = () =>
  invoke<SyncStatus>('get_sync_status');

/** Non-secret summary of the persisted adapter config. Returns
 *  `null` when no adapter is configured (or kind == none).
 *  Used by the Settings → Sync panel's "Verbunden mit X" card
 *  so the user sees what they're connected to without exposing
 *  the editable form. */
export interface SyncAdapterSummary {
  kind: string;
  detail: string;
  /** The account row this device syncs through. `null` on a device still
   *  reading the legacy `sync.adapter.*` preferences. The sync settings mark
   *  the chosen row in their account list by ID rather than by name — two
   *  accounts may carry the same display name. */
  account_id: string | null;
}
export const getSyncAdapterSummary = () =>
  invoke<SyncAdapterSummary | null>('get_sync_adapter_summary');

export const syncNow = () =>
  invoke<SyncRoundReport>('sync_now');

export const configureSyncAdapter = (config: SyncAdapterConfig) =>
  invoke<void>('configure_sync_adapter', { config });

/** Point this device at an account it ALREADY has, and sync through it.
 *
 *  The sync settings' one verb. The account was added under Settings →
 *  Accounts (or arrived with a restored dataset), so nothing here carries a
 *  host, a path or a password — the row already holds them.
 *
 *  Probes the target and checks the dataset's compatibility + encryption
 *  before writing anything down, so a refusal leaves this device syncing
 *  exactly where it did. Rejects with a stable `code`:
 *
 *   - `host_key_not_trusted` — confirm the server's fingerprint first, via
 *     [`previewSyncAccountHostKey`] + [`trustSftpHostKey`].
 *   - `plugin_missing` — no loaded plugin serves this account's adapter.
 *   - `auth` — the credential is not in this device's keychain.
 *   - `encryption_required` — the target holds an encrypted dataset this
 *     device has no key for; joining it is the onboarding flow's job.
 *   - `encryption_key_mismatch` — this device holds a key from an earlier
 *     setup that does not open this dataset. Re-send with `passphrase` to
 *     replace it; without one, nothing is changed.
 *   - `decryption_failed` — a `passphrase` was sent and does not open the
 *     dataset either. */
export const selectSyncAccount = (accountId: string, passphrase?: string) =>
  invoke<void>('select_sync_account', { accountId, passphrase: passphrase ?? null });

/** Update the periodic interval (in minutes). Clamps to ≥1 on the
 *  backend; returns the value actually persisted. */
export const setSyncInterval = (minutes: number) =>
  invoke<number>('set_sync_interval', { minutes });

/** Manual override for the auto-trigger compaction. The auto path
 *  fires inside `syncNow` whenever the §19.10 thresholds breach. */
export const compactNow = () =>
  invoke<CompactionReport>('compact_now');

// ── Onboarding flow (§19.11) ──

/** Whether this is a FRESH instance that should be offered the first-launch
 *  wizard: no external account, no sync target configured, and an empty local
 *  store. DATA-based (not a stored flag), so an established install is never
 *  prompted; the frontend pairs it with a device-local "already shown" marker. */
export const isFreshInstance = () => invoke<boolean>('is_fresh_instance');

/** Probe a remote without committing the orchestrator state.
 *  Returns `{kind: 'empty'}` for a fresh remote or `{kind: 'existing', ...}`
 *  with the device list + snapshot metadata. */
export const previewSyncTarget = (config: SyncAdapterConfig) =>
  invoke<SyncPreview>('preview_sync_target', { config });

/** "Datensatz übernehmen" — pull every remote log (and snapshot if
 *  present), apply locally, register this device in meta.json,
 *  configure the orchestrator. Pass `passphrase` when the preview
 *  shows `e2e_enabled = true`; the backend derives the AES key
 *  via Argon2id and stores it in the OS keychain. */
export const acceptRemoteDataset = (
  config: SyncAdapterConfig,
  deviceName: string | null,
  passphrase: string | null = null,
) =>
  invoke<OnboardingReport>('accept_remote_dataset', {
    config,
    deviceName,
    passphrase,
  });

/** "Neu beginnen" — overwrite the remote `meta.json` with one
 *  naming only this device. Caller is responsible for confirming
 *  the destructive action. Pass `passphrase` to enable E2E
 *  encryption on the fresh dataset; the backend mints fresh KDF
 *  params + stores the derived key in the OS keychain. */
export const adoptLocalDataset = (
  config: SyncAdapterConfig,
  deviceName: string | null,
  passphrase: string | null = null,
) =>
  invoke<OnboardingReport>('adopt_local_dataset', {
    config,
    deviceName,
    passphrase,
  });

/** The three onboarding calls again, asked with an adapter kind and the values
 *  the shared schema form produced.
 *
 *  Same flow and same answers as the typed trio above. What differs is what
 *  they leave behind on success: an account row and this device's pointer at
 *  it, so a device onboarded through the wizard looks afterwards exactly like
 *  one set up later in the settings.
 *
 *  Both sets exist while the two frontends move across one at a time. The
 *  typed half goes with the last per-backend form.
 *
 *  `previewSyncTargetValues` rejects with `host_key_not_trusted` for a target
 *  whose fingerprint this device has not confirmed — answer it with
 *  [`previewSftpHostKey`] + [`trustSftpHostKey`] and try again. */
export const previewSyncTargetValues = (
  kind: string,
  values: Record<string, string | boolean>,
) => invoke<SyncPreview>('preview_sync_target_values', { kind, values });

export const acceptRemoteDatasetValues = (
  kind: string,
  values: Record<string, string | boolean>,
  deviceName: string | null,
  passphrase: string | null = null,
) =>
  invoke<OnboardingReport>('accept_remote_dataset_values', {
    kind,
    values,
    deviceName,
    passphrase,
  });

export const adoptLocalDatasetValues = (
  kind: string,
  values: Record<string, string | boolean>,
  deviceName: string | null,
  passphrase: string | null = null,
) =>
  invoke<OnboardingReport>('adopt_local_dataset_values', {
    kind,
    values,
    deviceName,
    passphrase,
  });

/** §19.7 — rotate the dataset's E2E passphrase. Verifies the
 *  old passphrase via the wrap (or, on legacy v1 datasets, the
 *  direct-derived key), then rewraps the long-term data key
 *  under a new KEK + fresh salt. The blob ciphertext on the
 *  remote stays untouched, so other devices keep syncing
 *  without interruption — they only need the new passphrase
 *  when they re-onboard. Errors collapse to `auth` on a wrong
 *  current passphrase and `not_configured` when E2E isn't on.
 */
export const changeSyncPassphrase = (
  oldPassphrase: string,
  newPassphrase: string,
) =>
  invoke<void>('change_sync_passphrase', {
    oldPassphrase,
    newPassphrase,
  });

/** Counters returned by `disableSyncEncryption`. */
export interface DisableE2eReport {
  logs_rewritten: number;
  snapshot_rewritten: boolean;
}

/** §19.7 — turn off end-to-end encryption on the dataset. In
 *  place: re-uploads every log and the snapshot as plaintext via
 *  the bare adapter, then flips meta.json to e2e_enabled=false.
 *  Verifies the current passphrase first so an accidental click
 *  can't strip encryption. **Other devices need to re-onboard
 *  after this completes** — their keychain still holds the old
 *  DEK and they'll fail to decode the new plaintext. The UI
 *  gates this behind an explicit confirmation. */
export const disableSyncEncryption = (currentPassphrase: string) =>
  invoke<DisableE2eReport>('disable_sync_encryption', {
    currentPassphrase,
  });

/** Counters returned by `enableSyncEncryption`. */
export interface EnableE2eReport {
  logs_rewritten: number;
  snapshot_rewritten: boolean;
}

/** §19.7 — turn on end-to-end encryption for an existing,
 *  previously-unencrypted dataset. Mirror image of
 *  `disableSyncEncryption`: fetches every log + snapshot via
 *  the plain adapter, pushes them back via an encrypting
 *  wrapper, then flips meta.json to `e2e_enabled = true`. The
 *  passphrase becomes the dataset's KEK; other devices on the
 *  same dataset need to re-onboard with it after this
 *  completes. Maps to `conflict` if the dataset is already
 *  encrypted (race against another device). */
export const enableSyncEncryption = (newPassphrase: string) =>
  invoke<EnableE2eReport>('enable_sync_encryption', {
    newPassphrase,
  });

/** §19.7 — adopt encryption that was activated on another
 *  device. Pure unlock: derives the DEK from the passphrase +
 *  meta's e2e_params, stashes it locally, swaps the in-memory
 *  adapter over to the encrypting wrapper. Triggered from the
 *  cross-device banner that appears when sync_now fails with
 *  `encryption_required` on a previously-unencrypted dataset.
 *  Returns `auth` on a wrong passphrase. */
export const adoptRemoteEncryption = (passphrase: string) =>
  invoke<void>('adopt_remote_encryption', {
    passphrase,
  });

/** §19.10 stale-device resume. Called from the resume dialog
 *  when the user clicks Fortfahren. Re-pulls the current
 *  snapshot, applies it locally, replays post-snapshot logs,
 *  clears the device's `stale` flag. Returns an OnboardingReport
 *  shape so the dialog can show "12 events applied". */
export const resumeStaleDevice = () =>
  invoke<OnboardingReport>('resume_stale_device');

// ── Conflict resolution (§19.3) ──

export const listSyncConflicts = () =>
  invoke<SyncConflict[]>('list_sync_conflicts');

export const getSyncConflictsCount = () =>
  invoke<number>('get_sync_conflicts_count');

export const resolveSyncConflict = (
  id: number,
  choice: SyncResolutionChoice,
) => invoke<void>('resolve_sync_conflict', { id, choice });

// ── Sync protocol (§19.9 detailed history) ──

/** One row of the §19.9 Sync-Protokoll. Successful rounds
 *  carry the counter fields populated; failures carry
 *  `error` instead. */
export interface SyncLogEntry {
  id: number;
  /** RFC 3339 timestamp the scheduler recorded. */
  recorded_at: string;
  /** Why this round ran. One of:
   *  `app_start` | `periodic` | `kick` | `manual` | `app_exit`. */
  trigger: string;
  success: boolean;
  pushed_logs: number | null;
  fetched_logs: number | null;
  applied: number | null;
  conflicts: number | null;
  /** Wall-clock duration in milliseconds. */
  duration_ms: number | null;
  /** Set on `success = false`. Free-form error string. */
  error: string | null;
}

/** Read recent sync rounds from the protocol table, newest first.
 *  `limit` caps the returned set; values above the backend's
 *  retention ceiling (200 rows) are silently clamped. */
export const listSyncLogEntries = (limit?: number) =>
  invoke<SyncLogEntry[]>('list_sync_log_entries', {
    limit: limit ?? null,
  });

/** Drop every row from the protocol table. */
export const clearSyncLog = () => invoke<void>('clear_sync_log');

// ── Diagnostics log (Settings → Protokolle) ──

/** Log levels the diagnostics filter accepts, least → most verbose. */
export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

/** The persisted diagnostics log level (device-local). */
export const getLogLevel = () => invoke<string>('get_log_level');

/** Change the live verbosity and persist it. */
export const setLogLevel = (level: LogLevel) =>
  invoke<void>('set_log_level', { level });

/** Tail of the newest log file for the in-app viewer. */
export const getRecentLogs = (lines?: number) =>
  invoke<string>('get_recent_logs', { lines: lines ?? null });

/** The full (optionally redacted) log bundle as a string — for clipboard. */
export const collectLogs = (redact = true) =>
  invoke<string>('collect_logs', { redact });

/** Write the (optionally redacted) bundle to a user-chosen path. */
export const exportLogs = (destPath: string, redact = true) =>
  invoke<void>('export_logs', { destPath, redact });

/** Remove rotated log files (the active one is kept). */
export const clearLogs = () => invoke<void>('clear_logs');

/** The on-disk logs directory path, for display + copy. */
export const logsDirPath = () => invoke<string>('logs_dir_path');

// ── SFTP host-key trust dialog (§19.5) ──

/** Snapshot returned by `previewSftpHostKey`. The frontend uses
 *  `status.kind` to decide between the "first use" and "key
 *  changed" confirmation dialogs (or to skip the dialog entirely
 *  on `unchanged`). */
export interface HostKeyPreview {
  host_port: string;
  fingerprint: string;
  status: HostKeyPreviewStatus;
}

export type HostKeyPreviewStatus =
  | { kind: 'new' }
  | { kind: 'unchanged' }
  | { kind: 'changed'; stored: string };

/** Read the server's SHA256 host-key fingerprint without
 *  authenticating or pinning. Used by the SyncPanel before it
 *  configures an SFTP adapter so the user can verify the
 *  fingerprint on first use (or refuse a changed one). */
export const previewSftpHostKey = (host: string, port: number) =>
  invoke<HostKeyPreview>('preview_sftp_host_key', { host, port });

/** The same probe for an account the user is about to sync through, when
 *  [`selectSyncAccount`] refused with `host_key_not_trusted`.
 *
 *  Takes an account id rather than a host and a port because the sync settings
 *  have neither: the row holds them, and WHICH of its fields they are is the
 *  adapter's own `host_key_pin` declaration. Rejects with `invalid_input` for
 *  an account whose protocol does not pin host keys.
 *
 *  Accepting commits through the same [`trustSftpHostKey`] as the connect
 *  form — one pin store, keyed by `host:port`, so a fingerprint confirmed on
 *  either path is confirmed for both. */
export const previewSyncAccountHostKey = (accountId: string) =>
  invoke<HostKeyPreview>('preview_sync_account_host_key', { accountId });

/** One account's host-key pin, as this device holds it. */
export interface HostKeyPinInfo {
  /** The host the row names, empty when it names none. */
  host: string;
  /** The port as text — a number in newer rows, a string in older ones. */
  port: string;
  /** The `host:port` the pin is keyed by, `null` when the row does not say. */
  host_port: string | null;
  /** The confirmed fingerprint, `null` when this device has confirmed none. */
  fingerprint: string | null;
}

/** What this device has confirmed about an account's server, WITHOUT touching
 *  the network.
 *
 *  The probe above asks what the server is presenting now; this asks what the
 *  user already decided. That is what a screen offering to REVOKE the decision
 *  needs — asking the network for it would mean the trust cannot be withdrawn
 *  while the server is unreachable, which is exactly when withdrawing it
 *  matters.
 *
 *  `null` when the account's adapter pins no host keys. A `host_port` of `null`
 *  inside a value means the row does not say which server — a different thing,
 *  with a different repair. */
export const syncAccountHostKeyPin = (accountId: string) =>
  invoke<HostKeyPinInfo | null>('sync_account_host_key_pin', { accountId });

/** Pin a host-key fingerprint after the user has confirmed it
 *  in the trust dialog. Overwrites any prior pin for the same
 *  `host:port`, which is how the "key changed; accept new"
 *  flow commits its decision. */
export const trustSftpHostKey = (hostPort: string, fingerprint: string) =>
  invoke<void>('trust_sftp_host_key', { hostPort, fingerprint });

/** Drop the pinned fingerprint for a host_port. Used by the
 *  SyncPanel's "Pin vergessen" button — when the user knows the
 *  server's key was rotated, they clear the old pin so the next
 *  connect goes through the first-use trust dialog instead of
 *  firing the mismatch warning. */
export const forgetSftpHostKey = (hostPort: string) =>
  invoke<void>('forget_sftp_host_key', { hostPort });

/** §19.6 Dropbox OAuth dance. Opens the system browser at the
 *  Dropbox consent screen, blocks until the user completes (or
 *  the 5-minute timeout fires), stores the resulting refresh
 *  token in the keychain. Subsequent `configureSyncAdapter`
 *  calls with a Dropbox config can then build the adapter
 *  without prompting again. `clientSecret` is optional for
 *  public PKCE-only apps. */
export const connectDropboxOauth = (
  clientId: string,
  clientSecret: string,
) =>
  invoke<void>('connect_dropbox_oauth', { clientId, clientSecret });

/** Probe for whether the keychain already holds a Dropbox
 *  refresh token. Drives the "signed in" / "sign in" toggle on
 *  the Dropbox button in the SyncPanel. */
export const hasDropboxRefreshToken = () =>
  invoke<boolean>('has_dropbox_refresh_token');

/** Probe for whether the keychain already holds a Google
 *  Drive refresh token. Drives the "signed in" / "sign in"
 *  toggle on the Google Drive button in the SyncPanel. */
export const hasGoogledriveRefreshToken = () =>
  invoke<boolean>('has_googledrive_refresh_token');

/** Read the currently-pinned fingerprint for a host_port, or
 *  null if nothing is pinned. The SyncPanel uses this to render
 *  "Aktueller Pin: …" without probing the live server, so the
 *  pin-management UI works even when the server is unreachable. */
export const getPinnedSftpHostKey = (hostPort: string) =>
  invoke<string | null>('get_pinned_sftp_host_key', { hostPort });

// ── Plugins (§20.10) ──────────────────────────────────────────────

/** Snapshot of every plugin currently loaded into the host's
 *  PluginManager. The Settings → Plugins panel calls this to
 *  render the list. Sorted by id on the backend so re-fetches
 *  produce a stable order. */
export const listPlugins = () => invoke<PluginInfo[]>('list_plugins');

export interface SetPluginEnabledRequest {
  plugin_id: string;
  enabled: boolean;
}

/** Flip a plugin's enabled flag. Persists the new state in
 *  user_prefs + re-syncs the AdapterRegistry (accounts whose
 *  adapter_kind maps to the affected plugin get
 *  unregistered/re-registered in the same gesture). */
export const setPluginEnabled = (request: SetPluginEnabledRequest) =>
  invoke<void>('set_plugin_enabled', { request });

export interface InspectPluginArchiveRequest {
  archive_path: string;
}

/** Preview a `.aperio` archive's manifest without writing
 *  anything to disk. The install dialog uses this to render
 *  the unsigned-warning confirmation. */
export const inspectPluginArchive = (request: InspectPluginArchiveRequest) =>
  invoke<PluginArchivePreview>('inspect_plugin_archive', { request });

export interface InstallPluginArchiveRequest {
  archive_path: string;
}

/** Extract + load a `.aperio` archive. Returns the populated
 *  PluginInfo so the panel can splice it straight into the
 *  list. May return `restart_required` when the plugin id
 *  was already loaded (in-place upgrades need a restart for
 *  v1). */
export const installPluginArchive = (request: InstallPluginArchiveRequest) =>
  invoke<PluginInfo>('install_plugin_archive', { request });

export interface UninstallPluginRequest {
  plugin_id: string;
}

/** Drop a community plugin: drain in-flight calls, unload,
 *  scrub plugins/user/<id>/, clear the disabled flag.
 *  Refuses bundled plugins (`unsupported`) and the active
 *  sync plugin (`active_sync_conflict`). */
export const uninstallPlugin = (request: UninstallPluginRequest) =>
  invoke<void>('uninstall_plugin', { request });

/** Plugins other devices have announced via the cross-device
 *  Event Log (§20.8) but THIS device doesn't have installed
 *  locally. Used to render the "Plugin benötigt" section in
 *  the Settings → Plugins panel. */
export const listRemotePlugins = () =>
  invoke<RemotePluginAnnouncement[]>('list_remote_plugins');

/** Plugin directories the host's PluginManager refused to
 *  load at startup (ABI mismatch, malformed manifest,
 *  dlopen failure, …). Drives the "Konnten nicht geladen
 *  werden"-section of the Settings → Plugins panel. */
export const listFailedPlugins = () =>
  invoke<FailedPluginInfo[]>('list_failed_plugins');
