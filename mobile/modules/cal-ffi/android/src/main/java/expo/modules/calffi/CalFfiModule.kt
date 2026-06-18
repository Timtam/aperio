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
    Host.open(
      File(context.filesDir, "aperio.sqlite").absolutePath,
      AndroidKeychain(context),
    )
  }

  override fun definition() = ModuleDefinition {
    Name("CalFfi")

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

    AsyncFunction("updateTaskJson") { taskJson: String ->
      host.updateTaskJson(taskJson)
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

    AsyncFunction("getEventByIdJson") { id: String ->
      host.getEventByIdJson(id)
    }

    AsyncFunction("createEventJson") { requestJson: String ->
      host.createEventJson(requestJson)
    }

    AsyncFunction("updateEventJson") { eventJson: String ->
      host.updateEventJson(eventJson)
    }

    AsyncFunction("deleteEvent") { id: String, calendarId: String?, sendCancellations: Boolean? ->
      host.deleteEvent(id, calendarId, sendCancellations)
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

    AsyncFunction("syncNowJson") {
      host.syncNowJson()
    }

    AsyncFunction("pushNow") {
      host.pushNow().toInt()
    }

    // ─── Reminders ────────────────────────────────────────────────────────────
    // Upcoming reminder triggers (local + external) within a horizon, for the JS
    // layer to schedule as expo-notifications. `horizonMinutes` arrives as a JS
    // Number → widen the Int to the Rust u32.

    AsyncFunction("upcomingRemindersJson") { horizonMinutes: Int ->
      host.upcomingRemindersJson(horizonMinutes.toUInt())
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

    AsyncFunction("createContactListJson") { name: String ->
      host.createContactListJson(name)
    }

    AsyncFunction("deleteContactList") { id: String ->
      host.deleteContactList(id)
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
  }
}
