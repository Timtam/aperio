package expo.modules.calffi

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File
import uniffi.cal_ffi.Host
import uniffi.cal_ffi.parseAttendee as uniffiParseAttendee

class CalFfiModule : Module() {
  // The full on-device engine: tasks + lists + sections + accounts + the
  // statically-embedded adapter registry + cross-device sync, all over one
  // `<filesDir>/aperio.sqlite` (the same schema the desktop migrates). Tasks
  // used to be served by a separate `LocalStore`; they now fold into the Host
  // so every local mutation appends to the sync log and round-trips between
  // devices. Credentials route through AndroidKeychain (Keystore-backed
  // EncryptedSharedPreferences). `by lazy` is SYNCHRONIZED, so the concurrent
  // background threads `AsyncFunction` dispatches on share one Host safely; the
  // Rust engine serialises its own SQLite access behind a mutex.
  private val host: Host by lazy {
    val context = appContext.reactContext?.applicationContext
      ?: throw IllegalStateException(
        "CalFfi: no Android application context to resolve the data directory",
      )
    val opened = Host.open(
      File(context.filesDir, "aperio.sqlite").absolutePath,
      AndroidKeychain(context),
    )
    // Wire the external-cache observer to JS: a finished background refresh /
    // warm pass calls back here, and we forward it as an Expo event the RN layer
    // subscribes to (live-update the open view + a polite announcement).
    opened.setCacheObserver(JsCacheObserver())
    opened
  }

  /// Adapts the UniFFI `CacheObserverBridge` callback to Expo events. Fired on a
  /// background (tokio) thread; `sendEvent` marshals to JS.
  private inner class JsCacheObserver : uniffi.cal_ffi.CacheObserverBridge {
    override fun cacheUpdated(payloadJson: String) {
      this@CalFfiModule.sendEvent("onCacheUpdated", mapOf("payload" to payloadJson))
    }

    override fun refreshStatus(statusJson: String) {
      this@CalFfiModule.sendEvent("onCacheRefreshStatus", mapOf("status" to statusJson))
    }
  }

  override fun definition() = ModuleDefinition {
    Name("CalFfi")

    // External-cache push events (the mobile analogue of the desktop's Tauri
    // cache-updated / cache-refresh-status events). onCacheUpdated carries
    // { payload: "<CacheUpdatedPayload JSON>" }; onCacheRefreshStatus carries
    // { status: "<CacheRefreshStatus JSON>" }.
    Events("onCacheUpdated", "onCacheRefreshStatus")

    // Calls the Rust `cal_ffi::parse_attendee` through the UniFFI-generated
    // Kotlin bindings (uniffi/cal_ffi/cal_ffi.kt), backed by libcal_ffi.so.
    // This is the engine-reuse boundary: cal-core's parser runs in-process.
    Function("parseAttendee") { entry: String ->
      val parsed = uniffiParseAttendee(entry)
      mapOf(
        "name" to parsed.name,
        "email" to parsed.email,
      )
    }

    // ─── Tasks / lists / sections (JSON bridge, sync-logged) ─────────────────
    // The full task / list / section domain crosses as a JSON string in the
    // cal_core serde shape — identical to the desktop's Tauri payloads — so
    // this layer is a trivial passthrough: the mobile api-client parses the
    // JSON into the shared @aperio/shared types, keeping the marshalling in one
    // place. Each Host mutation appends the matching SyncEvent. All store ops
    // are `AsyncFunction`s: Expo dispatches them off the JS thread, and a thrown
    // StoreException rejects the JS promise.

    AsyncFunction("taskListsJson") {
      host.taskListsJson()
    }

    AsyncFunction("createTaskListJson") { name: String ->
      host.createTaskListJson(name)
    }

    AsyncFunction("reparentTaskListJson") { id: String, parentId: String? ->
      host.reparentTaskListJson(id, parentId)
    }

    AsyncFunction("deleteTaskList") { id: String ->
      host.deleteTaskList(id)
    }

    AsyncFunction("tasksJson") { listId: String ->
      host.tasksJson(listId)
    }

    AsyncFunction("taskJson") { id: String ->
      host.taskJson(id)
    }

    AsyncFunction("createTaskJson") { listId: String, newTaskJson: String ->
      host.createTaskJson(listId, newTaskJson)
    }

    AsyncFunction("updateTaskJson") { taskJson: String, previousListId: String? ->
      host.updateTaskJson(taskJson, previousListId)
    }

    AsyncFunction("deleteTask") { taskId: String, listId: String? ->
      host.deleteTask(taskId, listId)
    }

    AsyncFunction("sectionsJson") { listId: String ->
      host.sectionsJson(listId)
    }

    // `position` arrives from JS as a Number; the Rust signature takes a u32
    // (Kotlin UInt), which Expo can't coerce a JS Number to directly, so take
    // an Int and widen it here.
    AsyncFunction("createSectionJson") { listId: String, name: String, position: Int, colorLabel: String? ->
      host.createSectionJson(listId, name, position.toUInt(), colorLabel)
    }

    AsyncFunction("updateSectionJson") { sectionJson: String ->
      host.updateSectionJson(sectionJson)
    }

    AsyncFunction("deleteSection") { id: String, listId: String? ->
      host.deleteSection(id, listId)
    }

    // ─── Accounts (the full engine: external adapters + secrets) ─────────────
    // JSON passthrough in the cal_core/desktop wire shape, same convention as
    // the task bridge. create_account_json persists the row, stores the secret
    // via the keychain bridge, and registers the adapter; a thrown
    // StoreException rejects the JS promise.

    AsyncFunction("accountsJson") {
      host.accountsJson()
    }

    AsyncFunction("createAccountJson") { requestJson: String ->
      host.createAccountJson(requestJson)
    }

    AsyncFunction("deleteAccount") { accountId: String ->
      host.deleteAccount(accountId)
    }

    AsyncFunction("renameAccountJson") { id: String, newName: String ->
      host.renameAccountJson(id, newName)
    }

    AsyncFunction("listAccountsMissingCredentialsJson") {
      host.listAccountsMissingCredentialsJson()
    }

    AsyncFunction("setAccountSecret") { accountId: String, secret: String ->
      host.setAccountSecret(accountId, secret)
    }

    // ─── Calendars + events (the on-device adapters, local + external) ───────
    // JSON passthrough in the cal_core/desktop wire shape. Routing (local vs
    // external account) happens Rust-side in the Host; a thrown StoreException
    // rejects the JS promise with the typed error (NotFound / Conflict / Auth /
    // …) the mobile api-client maps.

    AsyncFunction("listCalendarsJson") {
      host.listCalendarsJson()
    }

    AsyncFunction("createCalendarJson") { requestJson: String ->
      host.createCalendarJson(requestJson)
    }

    AsyncFunction("deleteCalendar") { id: String ->
      host.deleteCalendar(id)
    }

    AsyncFunction("getEventsJson") { requestJson: String ->
      host.getEventsJson(requestJson)
    }

    AsyncFunction("getEventByIdJson") { id: String, calendarId: String? ->
      host.getEventByIdJson(id, calendarId)
    }

    AsyncFunction("createEventJson") { requestJson: String ->
      host.createEventJson(requestJson)
    }

    AsyncFunction("updateEventJson") { eventJson: String, previousCalendarId: String? ->
      host.updateEventJson(eventJson, previousCalendarId)
    }

    AsyncFunction("deleteEvent") { id: String, calendarId: String?, sendCancellations: Boolean? ->
      host.deleteEvent(id, calendarId, sendCancellations)
    }

    AsyncFunction("addEventExdateJson") { id: String, occurrence: String, calendarId: String? ->
      host.addEventExdateJson(id, occurrence, calendarId)
    }

    // ─── Sync (full desktop peer: same engine, statically-embedded adapters) ──
    // configure sets the active sync target; sync_now runs a round + returns the
    // report; a thrown StoreException rejects the JS promise.

    AsyncFunction("configureSyncAdapterJson") { configJson: String ->
      host.configureSyncAdapterJson(configJson)
    }

    AsyncFunction("syncStatusJson") {
      host.syncStatusJson()
    }

    AsyncFunction("syncNowJson") { trigger: String ->
      host.syncNowJson(trigger)
    }

    AsyncFunction("pushNow") { trigger: String ->
      host.pushNow(trigger).toInt()
    }

    AsyncFunction("listSyncLogJson") { limit: Int ->
      host.listSyncLogJson(limit.toUInt())
    }

    AsyncFunction("clearSyncLog") {
      host.clearSyncLog()
    }

    AsyncFunction("refreshExternalCache") {
      host.refreshExternalCache()
    }

    AsyncFunction("getCacheRefreshStatusJson") {
      host.getCacheRefreshStatusJson()
    }

    AsyncFunction("warmCacheOnForeground") {
      host.warmCacheOnForeground()
    }

    AsyncFunction("syncConflictCount") {
      host.syncConflictCount().toInt()
    }

    AsyncFunction("listSyncConflictsJson") {
      host.listSyncConflictsJson()
    }

    AsyncFunction("resolveSyncConflict") { id: Int, choice: String ->
      host.resolveSyncConflict(id.toLong(), choice)
    }

    // ─── Reminders ────────────────────────────────────────────────────────────
    // Upcoming reminder triggers (local + external) within a horizon, for the JS
    // layer to schedule as expo-notifications. `horizonMinutes` arrives as a JS
    // Number → widen the Int to the Rust u32.

    AsyncFunction("upcomingRemindersJson") { horizonMinutes: Int ->
      host.upcomingRemindersJson(horizonMinutes.toUInt())
    }

    // ─── User preferences (generic key/value; synced-key whitelist) ───────────
    // Opaque string values; a whitelisted key change appends a SettingsUpdated
    // sync event Rust-side so it propagates across devices.

    AsyncFunction("getUserPref") { key: String ->
      host.getUserPref(key)
    }

    AsyncFunction("setUserPref") { key: String, value: String ->
      host.setUserPref(key, value)
    }

    AsyncFunction("deleteUserPref") { key: String ->
      host.deleteUserPref(key)
    }

    // ─── Colour labels (app-wide palette; local-only, always synced) ──────────

    AsyncFunction("listColorLabelsJson") { ->
      host.listColorLabelsJson()
    }

    AsyncFunction("createColorLabelJson") { name: String, hex: String ->
      host.createColorLabelJson(name, hex)
    }

    AsyncFunction("getOrCreateAdHocColorLabelJson") { hex: String ->
      host.getOrCreateAdHocColorLabelJson(hex)
    }

    AsyncFunction("updateColorLabelJson") { labelJson: String ->
      host.updateColorLabelJson(labelJson)
    }

    AsyncFunction("deleteColorLabel") { id: String ->
      host.deleteColorLabel(id)
    }

    AsyncFunction("setContainerColorLabel") { containerId: String, kind: String, colorLabelId: String? ->
      host.setContainerColorLabel(containerId, kind, colorLabelId)
    }

    AsyncFunction("renameContainer") { containerId: String, kind: String, name: String ->
      host.renameContainer(containerId, kind, name)
    }

    AsyncFunction("setSectionColor") { sectionId: String, listId: String, colorLabelId: String? ->
      host.setSectionColor(sectionId, listId, colorLabelId)
    }

    AsyncFunction("setEventColor") { eventId: String, calendarId: String, colorLabelId: String? ->
      host.setEventColor(eventId, calendarId, colorLabelId)
    }

    AsyncFunction("searchJson") { query: String, filtersJson: String ->
      host.searchJson(query, filtersJson)
    }

    AsyncFunction("searchContactsJson") { query: String ->
      host.searchContactsJson(query)
    }

    // ─── Contacts (local address book + external CardDAV/Google/EWS providers) ─
    // JSON passthrough, routed Rust-side; contacts are NOT on the sync event log
    // (local = device-local, external = provider-synced).

    AsyncFunction("contactListsJson") {
      host.contactListsJson()
    }

    AsyncFunction("contactsJson") { listId: String ->
      host.contactsJson(listId)
    }

    AsyncFunction("createContactJson") { listId: String, contactJson: String ->
      host.createContactJson(listId, contactJson)
    }

    AsyncFunction("updateContactJson") { contactJson: String ->
      host.updateContactJson(contactJson)
    }

    AsyncFunction("deleteContact") { id: String, listId: String? ->
      host.deleteContact(id, listId)
    }

    AsyncFunction("getContactPhotoJson") { id: String, listId: String? ->
      host.getContactPhotoJson(id, listId)
    }

    AsyncFunction("setContactPhotoJson") { id: String, listId: String?, photoJson: String ->
      host.setContactPhotoJson(id, listId, photoJson)
    }

    AsyncFunction("deleteContactPhoto") { id: String, listId: String? ->
      host.deleteContactPhoto(id, listId)
    }

    AsyncFunction("createContactListJson") { name: String ->
      host.createContactListJson(name)
    }

    AsyncFunction("deleteContactList") { id: String ->
      host.deleteContactList(id)
    }

    // ─── Collaboration: RSVP (§7.3) + task-list members/sharing (§9.7) ────────
    // Routed Rust-side to the owning external adapter; reads degrade to empty /
    // null for local + unroutable accounts (the UI hides the affordance), writes
    // throw. respondToEvent invalidates the event cache so the next read shows
    // the new status.

    AsyncFunction("calendarCurrentUserEmail") { calendarId: String ->
      host.calendarCurrentUserEmail(calendarId)
    }

    AsyncFunction("respondToEvent") { calendarId: String, eventId: String, status: String, sendResponse: Boolean ->
      host.respondToEvent(calendarId, eventId, status, sendResponse)
    }

    AsyncFunction("taskListMembersJson") { listId: String ->
      host.taskListMembersJson(listId)
    }

    AsyncFunction("taskCurrentUserJson") { listId: String ->
      host.taskCurrentUserJson(listId)
    }

    AsyncFunction("taskListSharesJson") { listId: String ->
      host.taskListSharesJson(listId)
    }

    AsyncFunction("taskSearchUsersJson") { listId: String, query: String ->
      host.taskSearchUsersJson(listId, query)
    }

    AsyncFunction("taskAddMember") { listId: String, memberRef: String, right: String? ->
      host.taskAddMember(listId, memberRef, right)
    }

    AsyncFunction("taskRemoveMember") { listId: String, memberRef: String ->
      host.taskRemoveMember(listId, memberRef)
    }

    AsyncFunction("taskSetMemberRight") { listId: String, memberRef: String, right: String ->
      host.taskSetMemberRight(listId, memberRef, right)
    }

    // ─── OAuth (host-driven; mobile opens authorize_url in a native session) ──
    // beginOauthJson runs the pure authorize phase (no network) → returns
    // {authorize_url, pkce_verifier, state}. complete (network exchange + account
    // creation) follows in a later phase.

    AsyncFunction("beginOauthJson") { pluginId: String, argsJson: String ->
      host.beginOauthJson(pluginId, argsJson)
    }

    AsyncFunction("completeOauthJson") { pluginId: String, requestJson: String ->
      host.completeOauthJson(pluginId, requestJson)
    }

    AsyncFunction("completeOauthReconnectJson") { pluginId: String, accountId: String, requestJson: String ->
      host.completeOauthReconnectJson(pluginId, accountId, requestJson)
    }

    // ─── Discovery (EWS Autodiscover; host-driven, like the desktop) ──────────
    // discoverJson runs a plugin's endpoint discovery (EWS: {email, password} →
    // {ews_url, account_email}); the network call hits the provider, so a thrown
    // StoreException rejects the JS promise with the plugin's actionable message.

    AsyncFunction("discoverJson") { pluginId: String, argsJson: String ->
      host.discoverJson(pluginId, argsJson)
    }

    // ─── Sync-target OAuth (Dropbox / Google Drive) ───────────────────────────
    // completeSyncOauthJson exchanges the redirect's code for tokens (network)
    // and stores the refresh token in the adapter's keychain slot; the JS layer
    // then calls configureSyncAdapterJson({kind:"dropbox"|"googledrive", …}).

    AsyncFunction("completeSyncOauthJson") { pluginId: String, requestJson: String ->
      host.completeSyncOauthJson(pluginId, requestJson)
    }

    // ─── E2E sync encryption (§19.7) ──────────────────────────────────────────
    // enableSyncEncryptionJson turns on E2E for the configured target (mint key,
    // write the encrypted dataset, encrypt every subsequent round).

    AsyncFunction("enableSyncEncryptionJson") { passphrase: String ->
      host.enableSyncEncryptionJson(passphrase)
    }

    // disableSyncEncryptionJson turns E2E OFF: rewrites every log + snapshot as
    // plaintext, flips the meta, drops the device key (other devices re-onboard).
    AsyncFunction("disableSyncEncryptionJson") { passphrase: String ->
      host.disableSyncEncryptionJson(passphrase)
    }

    // changeSyncPassphraseJson rotates the E2E passphrase (re-wraps the same
    // data key; existing devices keep working, future joins need the new one).
    AsyncFunction("changeSyncPassphraseJson") { oldPassphrase: String, newPassphrase: String ->
      host.changeSyncPassphraseJson(oldPassphrase, newPassphrase)
    }

    // adoptRemoteEncryptionJson: a peer turned E2E on while this device synced
    // plaintext; derive the key from the passphrase + swap to an encrypting
    // adapter so the next round (which had failed with encryption_required) works.
    AsyncFunction("adoptRemoteEncryptionJson") { passphrase: String ->
      host.adoptRemoteEncryptionJson(passphrase)
    }

    // ─── Onboarding: preview + join an existing dataset (§19.11) ──────────────
    // previewSyncTargetJson reads the target's meta.json WITHOUT committing →
    // {kind: empty | existing, …}. acceptRemoteDatasetJson joins an existing
    // dataset (deriving the E2E key from the passphrase when it's encrypted).

    AsyncFunction("previewSyncTargetJson") { configJson: String ->
      host.previewSyncTargetJson(configJson)
    }

    AsyncFunction("acceptRemoteDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      host.acceptRemoteDatasetJson(configJson, deviceName, passphrase)
    }

    AsyncFunction("resumeStaleDeviceJson") {
      host.resumeStaleDeviceJson()
    }

    // ─── SFTP host-key trust (§19.5 TOFU) ─────────────────────────────────────
    // previewSftpHostKeyJson probes the server's fingerprint (network) + compares
    // it to the device pin store → {host_port, fingerprint, status}; trust/forget/
    // pinned manage the pin (no network). The JS layer shows the trust dialog.

    AsyncFunction("previewSftpHostKeyJson") { argsJson: String ->
      host.previewSftpHostKeyJson(argsJson)
    }

    AsyncFunction("trustSftpHostKey") { hostPort: String, fingerprint: String ->
      host.trustSftpHostKey(hostPort, fingerprint)
    }

    AsyncFunction("forgetSftpHostKey") { hostPort: String ->
      host.forgetSftpHostKey(hostPort)
    }

    AsyncFunction("pinnedSftpHostKey") { hostPort: String ->
      host.pinnedSftpHostKey(hostPort)
    }
  }
}
