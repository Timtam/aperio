import { NativeModule, requireNativeModule } from 'expo';

import { ParsedAttendee } from './CalFfi.types';

/** Native→JS events the module emits (the external-cache push). `payload` /
 *  `status` are JSON strings (a `CacheUpdatedPayload` / `CacheRefreshStatus`). */
export type CalFfiModuleEvents = {
  onCacheUpdated: (event: { payload: string }) => void;
  onCacheRefreshStatus: (event: { status: string }) => void;
  /** A contact-sync pass finished; `payload` is a `ContactsSyncedPayload` JSON. */
  onContactsSynced: (event: { payload: string }) => void;
};

declare class CalFfiModule extends NativeModule<CalFfiModuleEvents> {
  /**
   * Parse an attendee entry (`"Name <email>"` or a bare address) by calling
   * the shared Rust `cal-core` parser through UniFFI. Synchronous.
   */
  parseAttendee(entry: string): ParsedAttendee;

  // ── Tasks / lists / sections (JSON bridge, sync-logged) ──
  // The full task / list / section domain crosses as a JSON string in the
  // `cal_core` serde shape — identical to the desktop's Tauri payloads. Backed
  // by the Rust `Host`, so every local mutation appends to the sync log and
  // round-trips between devices. The mobile api-client parses these into the
  // shared `@aperio/shared` types; the bridge stays a dumb passthrough so the
  // marshalling lives in one place. Each rejects with the store's typed error
  // message on failure.

  /** All task lists as a JSON `TaskList[]`. */
  taskListsJson(): Promise<string>;
  /** Create a top-level local list; returns the created `TaskList` as JSON. */
  createTaskListJson(name: string): Promise<string>;
  /** Set or clear a list's parent (`null` promotes to top level); returns the
   *  updated `TaskList` as JSON. */
  reparentTaskListJson(id: string, parentId: string | null): Promise<string>;
  /** Delete a list (its tasks cascade away). Rejects on unknown id. */
  deleteTaskList(id: string): Promise<void>;
  /** Tasks in a list as a JSON `Task[]`, ordered by date then creation time. */
  tasksJson(listId: string): Promise<string>;
  /** One task by id as JSON. Rejects (not found) when absent. */
  taskJson(id: string): Promise<string>;
  /** Create a task from a JSON `NewTask`; returns the created `Task` as JSON. */
  createTaskJson(listId: string, newTaskJson: string): Promise<string>;
  /** Update a task from a JSON `Task`; returns the resulting `Task` as JSON.
   *  Completing a recurring task spawns its next instance (DESIGN §9.12).
   *  `previousListId` is the list the editor loaded the task FROM; when it
   *  differs from the task's `list_id` the bridge treats the save as a
   *  cross-list MOVE (create-on-target + best-effort-delete-from-source) so an
   *  external target isn't PATCHed at the wrong resource (412/404). A
   *  cross-adapter move returns the freshly-created task at the target (new id). */
  updateTaskJson(
    taskJson: string,
    previousListId: string | null,
  ): Promise<string>;
  /** Delete a task. `listId` (the owning list) routes the delete to the right
   *  account — omit/null for a local task. Rejects on unknown id. */
  deleteTask(taskId: string, listId: string | null): Promise<void>;
  /** Sections of a list as a JSON `Section[]`. */
  sectionsJson(listId: string): Promise<string>;
  /** Create a section; returns the created `Section` as JSON. */
  createSectionJson(
    listId: string,
    name: string,
    position: number,
    colorLabel: string | null,
  ): Promise<string>;
  /** Update a section from a JSON `Section`; returns it as JSON. */
  updateSectionJson(sectionJson: string): Promise<string>;
  /** Delete a section; its tasks fall back to ungrouped (`section_id` → null).
   *  `listId` (the owning list) routes the delete — omit/null for a local list. */
  deleteSection(id: string, listId: string | null): Promise<void>;

  // ── Accounts (the full engine: external adapters + secrets) ──
  // Backed by the Rust `Host` (statically-embedded adapter plugins + the
  // keychain-bridged SecretStore). Same JSON-passthrough convention as the
  // task bridge; each rejects with the store's typed error message on failure.

  /** All persisted accounts as a JSON `Account[]` (the desktop wire shape). */
  accountsJson(): Promise<string>;
  /**
   * Create an account from a JSON request (`adapter_kind`, `display_name`,
   * `config_json`, optional `secret`); persists the row, stores the secret via
   * the platform keychain, and registers the adapter. Returns the created
   * `Account` as JSON. Rejects for OAuth kinds (a later phase) + on bad config.
   */
  createAccountJson(requestJson: string): Promise<string>;
  /** Probe entered credentials WITHOUT persisting — opens an ephemeral adapter
   *  and runs the kind's read probe. Resolves if the creds work; rejects with
   *  the typed error otherwise. */
  testAccountJson(requestJson: string): Promise<void>;
  /** Delete an account: unregister its adapter, clear its secrets, drop the
   *  row. Rejects when deleting the implicit local account. */
  deleteAccount(accountId: string): Promise<void>;
  /** Rename an account's display name; returns the updated `Account` as JSON.
   *  Rejects on an empty name / unknown id. */
  renameAccountJson(id: string, newName: string): Promise<string>;
  /** Force a FULL cold re-sync of one external account: clear its delta tokens +
   *  cached window across every container, then kick a warm pass so each
   *  re-bootstraps from the provider. Cached rows stay as an offline fallback
   *  until replaced; credentials are untouched. The recovery action for a "stuck"
   *  external cache (a bootstrap that cached an incomplete set as complete). */
  resetAccountSync(accountId: string): Promise<void>;
  /** Run the OS calendar/reminders permission prompt for the device-calendar
   *  adapter's add-account "grant access" step. Resolves `true` iff access was
   *  granted (then create the `device_calendar` account). iOS-backed (EventKit);
   *  rejects "not available on this platform" on Android (no device bridge). */
  requestDeviceCalendarAccess(
    events: boolean,
    reminders: boolean,
  ): Promise<boolean>;
  /** External accounts whose required keychain secret is absent (the
   *  credential-repair banner data), as a JSON `Account[]`. */
  listAccountsMissingCredentialsJson(): Promise<string>;
  /** (Re-)store the secret half of a NON-OAuth account's credentials (CalDAV/EWS
   *  password or Vikunja/Todoist API token) and re-register its adapter. Rejects
   *  the local account, OAuth accounts (they must reconnect via OAuth), and an
   *  unknown id. */
  setAccountSecret(accountId: string, secret: string): Promise<void>;

  // ── Calendars + events ──
  // JSON passthrough in the cal_core/desktop wire shape; routing (local vs
  // external account) happens Rust-side in the Host. Rejects with the typed
  // store error (not_found / conflict / auth / …) on failure.

  /** All calendars (local + external) as a JSON `CalendarRow[]`. Also primes
   *  the Host's calendar→account route map, so call it before event ops. */
  listCalendarsJson(): Promise<string>;
  /** Create a local calendar from `{name, color_label?}`; returns the created
   *  `CalendarRow` as JSON. */
  createCalendarJson(requestJson: string): Promise<string>;
  /** Delete a local calendar (its events cascade). */
  deleteCalendar(id: string): Promise<void>;
  /** Events overlapping a range, from `{calendar_id, start, end}` (RFC-3339);
   *  returns a JSON `Event[]`. */
  getEventsJson(requestJson: string): Promise<string>;
  /** One event by id as JSON (`Event` or `null`). `calendarId` routes the
   *  lookup: a LOCAL calendar (or null) reads the stored row; an EXTERNAL one
   *  reads from the SWR snapshot cache (the adapter has no by-id fetch). */
  getEventByIdJson(id: string, calendarId: string | null): Promise<string>;
  /** Attendee free/busy over a window for the account owning the request's
   *  calendar, as a JSON `FreeBusy[]`. `{calendar_id, emails, range_start,
   *  range_end}`; best-effort → `[]` for local/unroutable/can't-answer. */
  queryFreeBusyJson(requestJson: string): Promise<string>;
  /** Create an event from `{calendar_id, …NewEvent}`; returns the created
   *  `Event` as JSON. */
  createEventJson(requestJson: string): Promise<string>;
  /** Update an event from a JSON `Event` (its `calendar_id` selects the route).
   *  `previousCalendarId` is the calendar the editor loaded the event FROM; when
   *  it differs from the event's `calendar_id` the bridge treats the save as a
   *  cross-calendar MOVE (create-on-target + best-effort-delete-from-source) so
   *  an external target isn't PUT to a non-existent resource (412). Returns the
   *  resulting `Event` as JSON. */
  updateEventJson(
    eventJson: string,
    previousCalendarId: string | null,
  ): Promise<string>;
  /** Delete an event. `calendarId` is routing-only (null → local);
   *  `sendCancellations` (external only) defaults to false. */
  deleteEvent(
    id: string,
    calendarId: string | null,
    sendCancellations: boolean | null,
  ): Promise<void>;
  /** Exclude ONE occurrence of a recurring event (append its RFC-3339 instant to
   *  the master's EXDATE) — the "this occurrence only" delete/edit primitive.
   *  `calendarId` routes (null → local). */
  addEventExdateJson(
    id: string,
    occurrence: string,
    calendarId: string | null,
  ): Promise<void>;

  // ── Sync (full desktop peer: same engine, statically-embedded adapters) ──
  /** Set the active sync target from a JSON `{kind, …}` (local: `{kind:"local",
   *  path}`); persists + probes. Rejects on a bad target / unsupported kind. */
  configureSyncAdapterJson(configJson: string): Promise<void>;
  /** The orchestrator status as JSON (configured / in_flight / last_synced_at /
   *  e2e_enabled / …). */
  syncStatusJson(): Promise<string>;
  /** Run one sync round (push + fetch + apply); returns the SyncRoundReport as
   *  JSON. Rejects "not configured" until a target is set. `trigger` is the wire
   *  SyncTrigger ("manual"/"app_start"/"periodic"/…) recorded in the sync log. */
  syncNowJson(trigger: string): Promise<string>;
  /** Disconnect the configured sync target (deconfigure + mark kind "none";
   *  keeps the field prefs + secrets so reconnect is one tap). */
  disconnectSync(): Promise<void>;
  /** Non-secret summary of the configured target as JSON (`null` or
   *  `{kind, detail}`) for the Settings "connected" card. */
  getSyncAdapterSummaryJson(): Promise<string>;
  /** Push local pending logs without fetching (RN AppState background); returns
   *  the number of logs pushed. `trigger` ("kick"/"app_exit"/…) is logged. */
  pushNow(trigger: string): Promise<number>;
  /** Recent sync-log rows (newest first) as a JSON `SyncLogEntry[]`, capped at
   *  `limit`. */
  listSyncLogJson(limit: number): Promise<string>;
  /** Drop every sync-log row. */
  clearSyncLog(): Promise<void>;
  /** Run a compaction round now (snapshot + GC old logs); resolves to a JSON
   *  `CompactionReport`. Rejects when no sync target is configured. */
  compactNowJson(): Promise<string>;
  /** Kick an immediate warm pass over every external account's containers +
   *  in-window events (the manual "refresh now"). Fire-and-forget. */
  refreshExternalCache(): Promise<void>;
  /** The external-cache warm-pass status as JSON `{refreshing, last_refreshed_at}`. */
  getCacheRefreshStatusJson(): Promise<string>;
  /** Warm the external cache on app-foreground (the mobile stand-in for the
   *  desktop periodic warm loop). Fire-and-forget. */
  warmCacheOnForeground(): Promise<void>;
  /** Run one contact-sync pass (§10.5) — warms every external book's cache.
   *  `includeReadOnly`: `null`/`undefined` reads the pref, `true`/`false`
   *  overrides. Resolves `false` if a pass was already in flight. */
  syncContactsNow(includeReadOnly: boolean | null): Promise<boolean>;
  /** Contact-sync status as JSON `{last_synced_at, interval_minutes, in_flight,
   *  include_read_only_on_sync}`. */
  getContactsSyncStatusJson(): Promise<string>;
  /** Persist the periodic contact-sync interval (minutes, clamped [1,1440]);
   *  resolves to the clamped value. Device-local. */
  setContactsSyncInterval(minutes: number): Promise<number>;
  /** Persist the "also pull read-only directories" toggle. Device-local. */
  setContactsIncludeReadOnlyOnSync(enabled: boolean): Promise<void>;
  /** Drop every external book's contact cache + reset "last synced"; resolves
   *  to the count of accounts invalidated. */
  clearContactsCache(): Promise<number>;
  /** The persisted log level (`error`…`trace`), or the default when unset. */
  getLogLevel(): Promise<string>;
  /** Validate → live-reload the filter → persist (device-local). */
  setLogLevel(level: string): Promise<void>;
  /** Tail of the newest log file (default 500 lines). `null` ⇒ default. */
  getRecentLogs(lines: number | null): Promise<string>;
  /** The full (optionally redacted, default true) log bundle, capped to ~2 MB
   *  for the Share sheet. */
  collectLogs(redact: boolean | null): Promise<string>;
  /** Remove the rotated log files (the active one is kept). */
  clearLogs(): Promise<void>;
  /** The on-disk logs directory, for display. */
  logsDirPath(): Promise<string>;
  /** Count of unresolved sync conflicts (the badge). */
  syncConflictCount(): Promise<number>;
  /** Every unresolved conflict as a JSON `ConflictRecord[]`. */
  listSyncConflictsJson(): Promise<string>;
  /** Resolve conflict `id`: `choice` is `'keep_local' | 'take_remote' |
   *  'save_both'` (save_both rejects — not supported yet). */
  resolveSyncConflict(id: number, choice: string): Promise<void>;

  // ── Reminders ──
  /** Upcoming reminder triggers (local + external) within `horizonMinutes` from
   *  now, as a JSON array of `{item_id, item_kind, title, body, trigger_at}`
   *  sorted ascending — for scheduling ahead-of-time OS local notifications. */
  upcomingRemindersJson(horizonMinutes: number): Promise<string>;

  // ── Custom reminder sounds (§14.4) ──
  /** Import an audio file (local `path` from the document picker) into the
   *  content-addressed store; returns JSON `{sha256, ext, path}`. Validates
   *  format + size Rust-side. */
  importSoundJson(path: string): Promise<string>;
  /** Every custom sound as JSON `[{sha256, ext, path}]`. */
  listCustomSoundsJson(): Promise<string>;
  /** Absolute on-disk path of a custom sound by hash, or null when not present
   *  locally (used for preview + the Android notification channel). */
  customSoundPath(sha256: string): Promise<string | null>;
  /** Delete a custom sound by hash (idempotent). */
  deleteCustomSound(sha256: string): Promise<void>;
  /** ANDROID: create (once) a NotificationChannel `channelId` whose sound is the
   *  custom audio file at `soundPath` (a FileProvider content URI), so an OS
   *  notification on that channel plays it. iOS: a no-op (the OS can't use a
   *  runtime file as a notification sound). Best-effort — the caller falls back
   *  to the default sound on rejection. */
  ensureCustomSoundChannel(
    channelId: string,
    soundPath: string,
    channelName: string,
  ): Promise<void>;

  // ── User preferences (generic key/value; synced-key whitelist) ──
  /** Read a user preference (opaque string), or null when unset. */
  getUserPref(key: string): Promise<string | null>;
  /** Upsert a user preference. A whitelisted key (locale, week-start, appearance,
   *  sound, default reminders, …) also syncs to the user's other devices. */
  setUserPref(key: string, value: string): Promise<void>;
  /** Delete a user preference (a whitelisted key also syncs the deletion). */
  deleteUserPref(key: string): Promise<void>;

  // ── Colour labels (app-wide palette; local-only, always synced) ──
  /** All colour labels (named + ad-hoc) as a JSON `ColorLabel[]`. */
  listColorLabelsJson(): Promise<string>;
  /** Create a named colour label; returns the created `ColorLabel` JSON. */
  createColorLabelJson(name: string, hex: string): Promise<string>;
  /** Resolve a one-off hex to a (deduped) ad-hoc colour label; returns it. */
  getOrCreateAdHocColorLabelJson(hex: string): Promise<string>;
  /** Update a colour label from a JSON `ColorLabel`; returns the updated one. */
  updateColorLabelJson(labelJson: string): Promise<string>;
  /** Delete a colour label by id. */
  deleteColorLabel(id: string): Promise<void>;
  /**
   * Set (or clear, with `null`) a LOCAL container's bound colour label. `kind`
   * is `'calendar' | 'task_list' | 'contact_list'`. Only local calendars / task
   * lists are supported (the binding rides their synced row); external
   * containers + contact lists (host-local overrides) reject until that path
   * lands.
   */
  setContainerColorLabel(
    containerId: string,
    kind: string,
    colorLabelId: string | null,
  ): Promise<void>;
  /**
   * Rename a LOCAL container. `kind` is `'calendar' | 'task_list' |
   * 'contact_list'`. Only local calendars / task lists are supported (the new
   * name rides their synced row); external containers + contact-list renames
   * (override path) reject until that lands.
   */
  renameContainer(containerId: string, kind: string, name: string): Promise<void>;
  /**
   * Set (or clear, with `null`) a SECTION's colour label. Routed by the owning
   * list's account: a local section carries it on its row, an external section
   * via a host-local override.
   */
  setSectionColor(
    sectionId: string,
    listId: string,
    colorLabelId: string | null,
  ): Promise<void>;
  /**
   * Set (or clear, with `null`) an external EVENT's host-local colour override.
   * A no-op for local / colour-capable calendars (the colour rides update_event
   * there). `eventId` is the series master id.
   */
  setEventColor(
    eventId: string,
    calendarId: string,
    colorLabelId: string | null,
  ): Promise<void>;

  // ── Search ──
  /**
   * Local full-text search over events + tasks; returns a JSON
   * `SearchResults { events, tasks }`. `filtersJson` is a JSON `SearchFilters`
   * or `''` for no filters.
   */
  searchJson(query: string, filtersJson: string): Promise<string>;
  /**
   * Cross-account contact search (local FTS + each external provider's search);
   * returns a JSON `Contact[]`, local hits first.
   */
  searchContactsJson(query: string): Promise<string>;

  // ── Contacts ──
  // JSON passthrough in the cal_core/desktop wire shape; routing (local vs
  // external account) happens Rust-side. Contacts are NOT on the sync event log.
  /** All address books (local + external) as a JSON `ContactListRow[]`. Primes
   *  the route map — call before contact ops. */
  contactListsJson(): Promise<string>;
  /** Contacts in a list as a JSON `Contact[]`, routed to the owning account. */
  contactsJson(listId: string): Promise<string>;
  /** Create a contact from a JSON `NewContact`; returns the created `Contact`. */
  createContactJson(listId: string, contactJson: string): Promise<string>;
  /** Update a contact from a JSON `Contact` (its `list_id` routes); returns it. */
  updateContactJson(contactJson: string): Promise<string>;
  /** Delete a contact. `listId` (the owning list) routes the delete — omit/null
   *  for a local contact. */
  deleteContact(id: string, listId: string | null): Promise<void>;
  /** A contact's avatar as JSON `Option<ContactPhoto>` (`{content_type,
   *  data:<base64>}` or `null`), routed by `listId` (null → local). */
  getContactPhotoJson(id: string, listId: string | null): Promise<string>;
  /** Set/replace a contact's avatar from a JSON `ContactPhoto`
   *  (`{content_type, data:<base64>}`), routed by `listId`. */
  setContactPhotoJson(
    id: string,
    listId: string | null,
    photoJson: string,
  ): Promise<void>;
  /** Remove a contact's avatar, routed by `listId`. */
  deleteContactPhoto(id: string, listId: string | null): Promise<void>;
  /** Create a local address book; returns the created `ContactListRow` as JSON. */
  createContactListJson(name: string): Promise<string>;
  /** Delete a local address book (the seeded default can't be deleted). */
  deleteContactList(id: string): Promise<void>;

  // ── Collaboration: RSVP (§7.3) + task-list members/sharing (§9.7) ──
  // Routed Rust-side to the owning external adapter; reads degrade to empty/null
  // for local + unroutable accounts (the UI hides the affordance), writes reject.
  /** The connected account's email for `calendarId` (RSVP "who am I"), or null
   *  for local/iCal calendars + providers that can't report an identity. */
  calendarCurrentUserEmail(calendarId: string): Promise<string | null>;
  /** RSVP to an invitation: set the connected user's participation `status`
   *  ('accepted' | 'tentative' | 'declined' | 'needs-action') on
   *  `calendarId`/`eventId`. `sendResponse` also emails the organizer on a
   *  scheduling-capable provider. Invalidates the event cache. */
  respondToEvent(
    calendarId: string,
    eventId: string,
    status: string,
    sendResponse: boolean,
  ): Promise<void>;
  /** Users assignable to a task in `listId` (its collaborator pool) as a JSON
   *  `TaskUser[]`. Empty for local lists / providers without sharing. */
  taskListMembersJson(listId: string): Promise<string>;
  /** The connected account's identity ("me") for `listId` as a JSON `TaskUser`
   *  (or `null`) — marks "assigned to me". `null` for local lists. */
  taskCurrentUserJson(listId: string): Promise<string>;
  /** The editable membership/shares of `listId` as a JSON `TaskListShare[]`.
   *  Empty for local / non-manageable backends. */
  taskListSharesJson(listId: string): Promise<string>;
  /** Search the owning account's user directory (Vikunja) for people to add to
   *  `listId`; returns a JSON `TaskUser[]`. Empty for backends without one. */
  taskSearchUsersJson(listId: string, query: string): Promise<string>;
  /** Add/invite a member to `listId`. `memberRef` is the provider's add key
   *  (Vikunja username, Todoist email); `right` ('read'|'write'|'admin'), or
   *  null where the backend has no roles. */
  taskAddMember(listId: string, memberRef: string, right: string | null): Promise<void>;
  /** Remove a member from `listId` (`memberRef` = the provider's remove key). */
  taskRemoveMember(listId: string, memberRef: string): Promise<void>;
  /** Change a member's `right` ('read'|'write'|'admin') on `listId` (Vikunja). */
  taskSetMemberRight(listId: string, memberRef: string, right: string): Promise<void>;

  // ── OAuth (host-driven; mobile opens authorize_url in a native session) ──
  /** Begin OAuth for an account plugin (e.g. `com.aperio.cal-adapter-google`).
   *  `argsJson` carries `{client_id, redirect_uri[, authority]}`. Returns the
   *  plugin's `{authorize_url, pkce_verifier, state}` JSON — the pure authorize
   *  phase (no network). The caller opens `authorize_url` in a native auth
   *  session and keeps the verifier/state for the matching exchange. */
  beginOauthJson(pluginId: string, argsJson: string): Promise<string>;
  /** Complete OAuth: exchange the redirect's code (+ pkce_verifier/state from
   *  begin) for tokens, then create + register the account. `requestJson` carries
   *  `{adapter_kind, display_name, config_json, client_id, client_secret?, code,
   *  pkce_verifier, state, returned_state, redirect_uri}`. Returns the created
   *  `Account` as JSON. */
  completeOauthJson(pluginId: string, requestJson: string): Promise<string>;
  /** Re-run OAuth for an EXISTING account (expired/lost token): exchange the
   *  code + write fresh tokens under `accountId` + re-register — no new row.
   *  `requestJson` is the same shape as completeOauthJson (its exchange fields
   *  are used; the kind comes from the account). Returns the account as JSON. */
  completeOauthReconnectJson(
    pluginId: string,
    accountId: string,
    requestJson: string,
  ): Promise<string>;

  // ── Discovery (EWS Autodiscover; host-driven) ──
  /** Run a plugin's endpoint discovery. `argsJson` carries `{email, password}`
   *  (EWS); returns the discovered endpoints as JSON (`{ews_url, account_email}`
   *  for EWS) to pre-fill the account form. The network call hits the provider's
   *  Autodiscover, so it rejects with the plugin's actionable message on
   *  failure — the caller can then fall back to a manually-entered endpoint. */
  discoverJson(pluginId: string, argsJson: string): Promise<string>;

  // ── Sync-target OAuth (Dropbox / Google Drive) ──
  /** Complete a host-driven OAuth flow for a SYNC adapter (`pluginId` =
   *  `com.aperio.sync-adapter-dropbox` / `…-googledrive`): exchange the
   *  redirect's code for tokens, then store the refresh token in the adapter's
   *  keychain slot. `requestJson` carries `{client_id, client_secret?, code,
   *  pkce_verifier, state, returned_state, redirect_uri}`. No account is created;
   *  the caller follows with `configureSyncAdapterJson` to activate the target. */
  completeSyncOauthJson(pluginId: string, requestJson: string): Promise<void>;

  // ── E2E sync encryption (§19.7) ──
  /** Enable end-to-end encryption on the configured sync target: mint a fresh
   *  passphrase-protected key, write the encrypted dataset (`adopt_local`), and
   *  encrypt every subsequent round. The key is device-local (never synced) — a
   *  second device joins via {@link acceptRemoteDatasetJson} + the passphrase.
   *  Returns the OnboardingReport JSON. */
  enableSyncEncryptionJson(passphrase: string): Promise<string>;
  /** Rotate the E2E passphrase: verify `oldPassphrase`, re-wrap the SAME data
   *  key under a fresh `newPassphrase` KEK + salt, push the updated `meta.json`.
   *  The data key is unchanged, so already-onboarded devices keep working; only
   *  future joins need the new passphrase. Rejects a non-encrypted target. */
  /** Disable E2E (§19.7): rewrite every log + snapshot as plaintext (stripping
   *  credential events/blocks), flip the meta to plaintext, and drop the device
   *  key. Other devices must re-onboard afterwards. Returns the
   *  `{logs_rewritten, snapshot_rewritten}` report as JSON. */
  disableSyncEncryptionJson(passphrase: string): Promise<string>;
  changeSyncPassphraseJson(
    oldPassphrase: string,
    newPassphrase: string,
  ): Promise<void>;
  /** Adopt encryption a peer turned on (§19.7): this device synced the dataset
   *  in plaintext, a peer enabled E2E, and the next round failed with
   *  `encryption_required`. Derives the key from `passphrase` + meta params,
   *  swaps to an encrypting adapter, flips the local pref, and re-emits local
   *  account secrets. The following round then applies the dataset decrypted. */
  adoptRemoteEncryptionJson(passphrase: string): Promise<void>;

  // ── Onboarding: preview + join an existing dataset (§19.11) ──
  /** Probe a sync target WITHOUT committing: build the adapter from `configJson`,
   *  read its `meta.json`, and return a `SyncPreview` JSON — `{kind:"empty"}` for
   *  a fresh target or `{kind:"existing", e2e_enabled, devices, …}` for one that
   *  already holds a dataset. Side-effect-free; the caller offers join vs
   *  overwrite. */
  previewSyncTargetJson(configJson: string): Promise<string>;
  /** Join an EXISTING remote dataset: build the adapter, derive the E2E key from
   *  `passphrase` + the dataset's meta params when it's encrypted, pull + apply
   *  the snapshot + logs, register this device, then activate + persist (storing
   *  the derived key device-locally). The only way a second device obtains the
   *  key for a foreign encrypted dataset. Returns the OnboardingReport JSON. */
  acceptRemoteDatasetJson(
    configJson: string,
    deviceName: string | null,
    passphrase: string | null,
  ): Promise<string>;
  /** "Start fresh" (§19.11): overwrite the target's meta.json so it names ONLY
   *  this device, optionally minting E2E from `passphrase` (null/blank = a
   *  plaintext fresh dataset), then activate + persist. The unified Connect
   *  button uses it to INITIALISE an empty target. Returns the OnboardingReport
   *  JSON. */
  adoptLocalDatasetJson(
    configJson: string,
    deviceName: string | null,
    passphrase: string | null,
  ): Promise<string>;
  /** Resume a STALE device (§19.10): re-onboard from the configured target +
   *  clear the latched stale flag. Returns the OnboardingReport JSON. Rejects
   *  when no target is configured. */
  resumeStaleDeviceJson(): Promise<string>;

  // ── SFTP host-key trust (§19.5 TOFU) ──
  /** Probe an SFTP server's SHA256 host-key fingerprint (network) and classify
   *  it against the device pin store. `argsJson` carries `{host, port}`; returns
   *  `{host_port, fingerprint, status}` JSON (status: `{kind:"new"|"unchanged"}`
   *  or `{kind:"changed", stored}`). The caller shows the trust dialog. */
  previewSftpHostKeyJson(argsJson: string): Promise<string>;
  /** Pin a user-confirmed fingerprint for `hostPort` (first-use or key-change). */
  trustSftpHostKey(hostPort: string, fingerprint: string): Promise<void>;
  /** Drop the pinned fingerprint for `hostPort` (next connect re-prompts). */
  forgetSftpHostKey(hostPort: string): Promise<void>;
  /** The pinned fingerprint for `hostPort`, or null. No network. */
  pinnedSftpHostKey(hostPort: string): Promise<string | null>;
}

export default requireNativeModule<CalFfiModule>('CalFfi');
