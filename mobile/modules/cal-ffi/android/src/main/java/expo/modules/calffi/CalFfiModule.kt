package expo.modules.calffi

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.os.Build
import android.util.Log
import androidx.core.content.FileProvider
// An extension function on GlanceAppWidget — see AperioWidget.kt.
import androidx.glance.appwidget.updateAll
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.modules.ModuleDefinitionBuilder
import java.io.File
import java.util.concurrent.Executors
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import expo.modules.kotlin.exception.CodedException
import uniffi.cal_ffi.Host
import uniffi.cal_ffi.StoreException
import uniffi.cal_ffi.parseAttendee as uniffiParseAttendee

class CalFfiModule : Module() {
  // Expo runs EVERY AsyncFunction body on ONE single-threaded HandlerThread
  // dispatcher ("expo.modules.AsyncFunctionQueue"), so a seconds-long sync
  // round makes every read call queue behind it — on app open the views then
  // sit on "Loading…" until the launch sync finishes, even with a warm SWR
  // cache. The long NETWORK-bound operations therefore run on their own
  // single-thread scope: sync rounds stay mutually exclusive among themselves,
  // while the short reads/writes on the default queue flow past them. (The
  // Rust Host is Send+Sync — uniffi's contract — and its SQLite access is
  // mutex-guarded, so both dispatchers may safely call into it concurrently.)
  // Mirrors the iOS module's `slowQueue`.
  // The dispatcher is kept separately so OnDestroy can close it — expo-modules
  // only cancels its OWN queues on teardown, so without this every React-host
  // re-creation (dev reload) would leak one "calffi.slow" thread.
  private val slowDispatcher =
    Executors.newSingleThreadExecutor { r -> Thread(r, "calffi.slow") }
      .asCoroutineDispatcher()
  private val slowScope = CoroutineScope(
    slowDispatcher + SupervisorJob() + CoroutineName("calffi.slow"),
  )

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
    // Every failure below is routed through NativeBridgeDiagnosis, because this
    // is the first thing in the process to touch the UniFFI bindings — and the
    // first touch is the only one that carries a usable error. If `UniffiLib`'s
    // static initialiser throws here, the JVM reports it once and then answers
    // every later attempt with a bare `NoClassDefFoundError` that names nothing.
    // `by lazy` re-runs on failure, so without this the second call is already
    // the uninformative one.
    try {
      openHost()
    } catch (error: Throwable) {
      // Only a bindings failure is recorded. A first attempt that fails for an
      // unrelated reason — a corrupt database, a missing Android context — must
      // travel on untouched, or its text would be pinned as THE diagnosis and
      // the retry's real cause would never be seen.
      if (!NativeBridgeDiagnosis.isBridgeFailure(error)) throw error
      throw IllegalStateException(NativeBridgeDiagnosis.record(error), error)
    }
  }

  private fun openHost(): Host {
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
    // Wire the contact-sync observer to JS: a finished pass forwards its payload
    // as an Expo event the RN layer subscribes to (update the "last synced"
    // footer + re-read the contact views).
    opened.setContactSyncObserver(JsContactSyncObserver())
    // Install the device calendar bridge (CalendarProvider). Android has no
    // system Reminders app, so it's calendar-only (supportsReminders=false). The
    // Host registers any persisted device-calendar account against it. Mirrors
    // the iOS module's IosDeviceEventStore injection.
    opened.setDeviceEventStore(AndroidDeviceCalendar(context))
    return opened
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

  /// Adapts the UniFFI `ContactSyncObserverBridge` callback to an Expo event.
  /// Fired on a background thread when a contact-sync pass finishes; `sendEvent`
  /// marshals to JS. Mirrors `JsCacheObserver`.
  private inner class JsContactSyncObserver : uniffi.cal_ffi.ContactSyncObserverBridge {
    override fun contactsSynced(payloadJson: String) {
      this@CalFfiModule.sendEvent("onContactsSynced", mapOf("payload" to payloadJson))
    }
  }

  /**
   * Re-throw a sync failure with its code intact.
   *
   * A `StoreException.Sync` carries the engine's own stable code, but Expo
   * turns an unmapped exception into a JS error whose message is Kotlin's
   * `toString()` — "code=auth, detail=..." — which the JS side can only show
   * verbatim. So the phone printed the engine's English while the desktop,
   * branching on that same code, showed a translated sentence.
   *
   * `CodedException` is the shape Expo surfaces as `error.code`, so the mobile
   * frontend can map it exactly the way `useSyncErrorMessage` does on desktop.
   * Everything else falls through untouched — a `StoreException.NotFound` was
   * never the problem.
   */
  private inline fun <T> coded(block: () -> T): T =
    try {
      block()
    } catch (e: StoreException.Sync) {
      throw CodedException(e.code, e.detail, e)
    }

  /**
   * Re-throw the ONE refusal a grouping request can meet with a code.
   *
   * `Conflict` is generic across the store, but at THIS call site it can only
   * be one thing: both named events are already in different groups. The
   * frontend has to say that sentence — "take one of them out first" — and it
   * cannot, if all that arrives is an exception's `toString()`.
   */
  private inline fun <T> groupCoded(block: () -> T): T =
    try {
      block()
    } catch (e: StoreException.Conflict) {
      throw CodedException("event_group_conflict", e.detail, e)
    }

  // Split across several `ModuleDefinitionBuilder` extensions rather than one
  // lambda. The JVM caps a single method's bytecode at 64 KB, and 139 function
  // registrations went past it: "Method too large:
  // CalFfiModule.definition()". Each extension compiles to its own method, so
  // the ceiling applies per group instead of to the module as a whole.
  override fun definition() = ModuleDefinition {
    Name("CalFfi")

    // External-cache push events (the mobile analogue of the desktop's Tauri
    // cache-updated / cache-refresh-status events). onCacheUpdated carries
    // { payload: "<CacheUpdatedPayload JSON>" }; onCacheRefreshStatus carries
    // { status: "<CacheRefreshStatus JSON>" }.
    Events("onCacheUpdated", "onCacheRefreshStatus", "onContactsSynced")

    // Eager engine open: the expensive Host construction (DB open +
    // migrations + plugin registrations + tokio runtime + orchestrator
    // build) used to hide inside the FIRST bridge call via `by lazy` —
    // every later call on the shared AsyncFunction dispatcher waited
    // behind it. Kick it on the slow scope while the JS bundle is still
    // loading; `by lazy` is synchronized, so a racing first call simply
    // waits for the remainder. Mirrors the iOS module's OnCreate.
    OnCreate {
      slowScope.launch {
        runCatching { host }.onFailure {
          // Do not widen this into a rethrow: the retry on the first real call
          // is the point of the eager open. The reason is not lost by swallowing
          // it here — NativeBridgeDiagnosis has already recorded and logged it,
          // and the retry will surface that same recorded text to the caller
          // rather than the `NoClassDefFoundError` the JVM would otherwise give.
          Log.w("CalFfi", "eager host open failed; first call will retry", it)
        }
      }
    }

    // Release the slow-ops thread when the module is torn down (dev reload /
    // React-host restart) — expo-modules cancels only its own queues.
    OnDestroy {
      slowScope.cancel()
      slowDispatcher.close()
    }

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

    AsyncFunction("testAccountJson") { requestJson: String ->
      host.testAccountJson(requestJson)
    }

    AsyncFunction("testAccountValuesJson") { requestJson: String ->
      host.testAccountValuesJson(requestJson)
    }

    AsyncFunction("runAccountActionJson") { requestJson: String ->
      host.runAccountActionJson(requestJson)
    }.runOnQueue(slowScope)

    AsyncFunction("deleteAccount") { accountId: String ->
      host.deleteAccount(accountId)
    }

    AsyncFunction("renameAccountJson") { id: String, newName: String ->
      host.renameAccountJson(id, newName)
    }

    // Uniform JS surface with iOS. Android installs no device-calendar bridge
    // (no system reminders app; the calendar adapter is iOS-first), so the Host
    // rejects "not available on this platform" — the UI gates the device picker
    // entry to iOS, so this is only ever reached defensively.
    AsyncFunction("requestDeviceCalendarAccess") { events: Boolean, reminders: Boolean ->
      host.requestDeviceCalendarAccess(events, reminders)
    }

    // Force a full cold re-sync of one external account (clears its delta tokens
    // + cached window, then kicks a warm pass). The recovery action for a "stuck"
    // external cache; credentials are untouched.
    AsyncFunction("resetAccountSync") { accountId: String ->
      host.resetAccountSync(accountId)
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

    AsyncFunction("queryFreeBusyJson") { requestJson: String ->
      host.queryFreeBusyJson(requestJson)
    }.runOnQueue(slowScope)

    AsyncFunction("createEventJson") { requestJson: String ->
      host.createEventJson(requestJson)
    }

    AsyncFunction("updateEventJson") { eventJson: String, previousCalendarId: String? ->
      host.updateEventJson(eventJson, previousCalendarId)
    }

    AsyncFunction("deleteEvent") { id: String, calendarId: String?, sendCancellations: Boolean? ->
      host.deleteEvent(id, calendarId, sendCancellations)
    }

    AsyncFunction("addEventExdateJson") { id: String, occurrence: String, calendarId: String?, sendCancellations: Boolean ->
      host.addEventExdateJson(id, occurrence, calendarId, sendCancellations)
    }

    syncFunctions()

    // ─── Reminders ────────────────────────────────────────────────────────────
    // Upcoming reminder triggers (local + external) within a horizon, for the JS
    // layer to schedule as expo-notifications. `horizonMinutes` arrives as a JS
    // Number → widen the Int to the Rust u32.

    AsyncFunction("upcomingRemindersJson") { horizonMinutes: Int ->
      host.upcomingRemindersJson(horizonMinutes.toUInt())
    }

    soundFunctions()

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

    AsyncFunction("listDayMarkersJson") { ->
      host.listDayMarkersJson()
    }

    AsyncFunction("createDayMarkerJson") { name: String, symbol: String?, colorLabel: String? ->
      host.createDayMarkerJson(name, symbol, colorLabel)
    }

    AsyncFunction("updateDayMarkerJson") { markerJson: String ->
      host.updateDayMarkerJson(markerJson)
    }

    AsyncFunction("deleteDayMarker") { id: String ->
      host.deleteDayMarker(id)
    }

    AsyncFunction("dayLogJson") { day: String ->
      host.dayLogJson(day)
    }

    AsyncFunction("dayLogsInRangeJson") { from: String, to: String ->
      host.dayLogsInRangeJson(from, to)
    }

    AsyncFunction("setDayLogJson") { logJson: String ->
      host.setDayLogJson(logJson)
    }

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

    AsyncFunction("groupEventsJson") { membersJson: String ->
      groupCoded { host.groupEventsJson(membersJson) }
    }

    AsyncFunction("ungroupEventJson") { calendarId: String, eventId: String, bookkeeping: Boolean ->
      host.ungroupEventJson(calendarId, eventId, bookkeeping)
    }

    AsyncFunction("dissolveEventGroup") { groupId: String ->
      host.dissolveEventGroup(groupId)
    }

    AsyncFunction("eventGroupsForEventsJson") { eventsJson: String ->
      host.eventGroupsForEventsJson(eventsJson)
    }

    AsyncFunction("refreshEventGroupSignature") {
      calendarId: String, eventId: String, title: String, startsAt: String ->
      host.refreshEventGroupSignature(calendarId, eventId, title, startsAt)
    }

    AsyncFunction("declineGroupSuggestionJson") { firstJson: String, secondJson: String ->
      host.declineGroupSuggestionJson(firstJson, secondJson)
    }

    AsyncFunction("groupSuggestionDeclinesJson") { ->
      host.groupSuggestionDeclinesJson()
    }

    AsyncFunction("healEventGroupMember") {
      groupId: String, calendarId: String, oldEventId: String, newEventId: String ->
      host.healEventGroupMember(groupId, calendarId, oldEventId, newEventId)
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

    contactFunctions()

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

    // ─── Schema-driven accounts ──────────────────────────────────────────────
    // The generic connect path: the adapter declares its form in its
    // plugin.json and the host executes the declaration, so adding an adapter
    // adds no code here either.

    meetingFunctions()

    // ─── OAuth (host-driven; mobile opens authorize_url in a native session) ──
    // beginOauthJson runs the pure authorize phase (no network) → returns
    // {authorize_url, pkce_verifier, state}. complete (network exchange + account
    // creation) follows in a later phase.

    AsyncFunction("beginOauthJson") { pluginId: String, argsJson: String ->
      host.beginOauthJson(pluginId, argsJson)
    }



    // ─── Discovery (EWS Autodiscover; host-driven, like the desktop) ──────────
    // discoverJson runs a plugin's endpoint discovery (EWS: {email, password} →
    // {ews_url, account_email}); the network call hits the provider, so a thrown
    // StoreException rejects the JS promise with the plugin's actionable message.

    AsyncFunction("discoverJson") { pluginId: String, argsJson: String ->
      host.discoverJson(pluginId, argsJson)
    }.runOnQueue(slowScope)

    // ─── Sync-target OAuth (Dropbox / Google Drive) ───────────────────────────
    // completeSyncOauthJson exchanges the redirect's code for tokens (network)
    // and stores the refresh token in the adapter's keychain slot; the JS layer
    // then calls configureSyncAdapterJson({kind:"dropbox"|"googledrive", …}).

    AsyncFunction("completeSyncOauthJson") { pluginId: String, requestJson: String ->
      host.completeSyncOauthJson(pluginId, requestJson)
    }.runOnQueue(slowScope)

    // ─── E2E sync encryption (§19.7) ──────────────────────────────────────────
    // enableSyncEncryptionJson turns on E2E for the configured target (mint key,
    // write the encrypted dataset, encrypt every subsequent round).

    AsyncFunction("enableSyncEncryptionJson") { passphrase: String ->
      coded { host.enableSyncEncryptionJson(passphrase) }
    }.runOnQueue(slowScope)

    // disableSyncEncryptionJson turns E2E OFF: rewrites every log + snapshot as
    // plaintext, flips the meta, drops the device key (other devices re-onboard).
    AsyncFunction("disableSyncEncryptionJson") { passphrase: String ->
      coded { host.disableSyncEncryptionJson(passphrase) }
    }.runOnQueue(slowScope)

    // changeSyncPassphraseJson rotates the E2E passphrase (re-wraps the same
    // data key; existing devices keep working, future joins need the new one).
    AsyncFunction("changeSyncPassphraseJson") { oldPassphrase: String, newPassphrase: String ->
      coded { host.changeSyncPassphraseJson(oldPassphrase, newPassphrase) }
    }.runOnQueue(slowScope)

    // adoptRemoteEncryptionJson: a peer turned E2E on while this device synced
    // plaintext; derive the key from the passphrase + swap to an encrypting
    // adapter so the next round (which had failed with encryption_required) works.
    AsyncFunction("adoptRemoteEncryptionJson") { passphrase: String ->
      coded { host.adoptRemoteEncryptionJson(passphrase) }
    }.runOnQueue(slowScope)

    onboardingFunctions()

    sftpFunctions()

    widgetFunctions()
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.syncFunctions() {
    // ─── Sync (full desktop peer: same engine, statically-embedded adapters) ──
    // configure sets the active sync target; sync_now runs a round + returns the
    // report; a thrown StoreException rejects the JS promise.

    AsyncFunction("configureSyncAdapterJson") { configJson: String ->
      coded { host.configureSyncAdapterJson(configJson) }
    }.runOnQueue(slowScope)

    // The sync SCREEN's verb: point this device at an account it already has.
    // Probes the target before it commits, so it belongs on the slow queue with
    // the other network-touching calls.
    AsyncFunction("selectSyncAccount") { accountId: String ->
      coded { host.selectSyncAccount(accountId) }
    }.runOnQueue(slowScope)

    AsyncFunction("syncStatusJson") {
      coded { host.syncStatusJson() }
    }

    AsyncFunction("syncNowJson") { trigger: String ->
      coded { host.syncNowJson(trigger) }
    }.runOnQueue(slowScope)

    AsyncFunction("disconnectSync") {
      host.disconnectSync()
    }

    AsyncFunction("getSyncAdapterSummaryJson") {
      host.getSyncAdapterSummaryJson()
    }

    AsyncFunction("pushNow") { trigger: String ->
      host.pushNow(trigger).toInt()
    }.runOnQueue(slowScope)

    AsyncFunction("listSyncLogJson") { limit: Int ->
      host.listSyncLogJson(limit.toUInt())
    }

    AsyncFunction("clearSyncLog") {
      host.clearSyncLog()
    }

    AsyncFunction("compactNowJson") {
      coded { host.compactNowJson() }
    }.runOnQueue(slowScope)

    AsyncFunction("refreshExternalCache") {
      host.refreshExternalCache()
    }

    AsyncFunction("getCacheRefreshStatusJson") {
      host.getCacheRefreshStatusJson()
    }

    // Per-account refresh-error surface (silent-staleness warning).
    AsyncFunction("refreshErrorsJson") {
      host.refreshErrorsJson()
    }

    AsyncFunction("warmCacheOnForeground") {
      host.warmCacheOnForeground()
    }

    // Contact sync (§10.5). The pass is driven from JS (manual button /
    // foreground); the interval + include-read-only prefs are device-local.
    AsyncFunction("syncContactsNow") { includeReadOnly: Boolean? ->
      coded { host.syncContactsNow(includeReadOnly) }
    }.runOnQueue(slowScope)

    AsyncFunction("getContactsSyncStatusJson") {
      host.getContactsSyncStatusJson()
    }

    AsyncFunction("setContactsSyncInterval") { minutes: Int ->
      host.setContactsSyncInterval(minutes.toUInt()).toInt()
    }

    AsyncFunction("setContactsIncludeReadOnlyOnSync") { enabled: Boolean ->
      host.setContactsIncludeReadOnlyOnSync(enabled)
    }

    AsyncFunction("clearContactsCache") {
      host.clearContactsCache().toInt()
    }

    // Diagnostics / logs (§ Diagnostics).
    AsyncFunction("getLogLevel") {
      host.getLogLevel()
    }

    AsyncFunction("setLogLevel") { level: String ->
      host.setLogLevel(level)
    }

    AsyncFunction("getRecentLogs") { lines: Int? ->
      host.getRecentLogs(lines?.toUInt())
    }

    AsyncFunction("collectLogs") { redact: Boolean? ->
      host.collectLogs(redact)
    }

    AsyncFunction("clearLogs") {
      host.clearLogs()
    }

    AsyncFunction("logsDirPath") {
      host.logsDirPath()
    }

    AsyncFunction("syncConflictCount") {
      coded { host.syncConflictCount().toInt() }
    }

    AsyncFunction("listSyncConflictsJson") {
      host.listSyncConflictsJson()
    }

    AsyncFunction("resolveSyncConflict") { id: Int, choice: String ->
      host.resolveSyncConflict(id.toLong(), choice)
    }
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.soundFunctions() {
    // ─── Custom reminder sounds (§14.4 / §19.2.2) ─────────────────────────────
    // Content-addressed audio store behind SoundSource::Custom; the sync round
    // push/fetches it already. Bytes don't cross the bridge — the JS plays +
    // builds the Android notification channel from the on-disk path.

    AsyncFunction("importSoundJson") { path: String ->
      host.importSoundJson(path)
    }

    AsyncFunction("listCustomSoundsJson") { ->
      host.listCustomSoundsJson()
    }

    AsyncFunction("customSoundPath") { sha256: String ->
      host.customSoundPath(sha256)
    }

    AsyncFunction("deleteCustomSound") { sha256: String ->
      host.deleteCustomSound(sha256)
    }

    // Create (once) a NotificationChannel whose sound is a user-imported custom
    // audio file. expo-notifications resolves only build-time res/raw sounds, so
    // a runtime file needs its own channel pointing at a FileProvider content://
    // URI the system-UI process can read. The channel id is stable per sound (the
    // sha), so this is create-once — Android makes the channel sound immutable
    // after creation. NOT a Rust/UniFFI call: pure Android. Any failure throws →
    // the JS scheduler catches it and falls back to the default sound. iOS can't
    // do this (UNNotificationSound is build-time-bundled), so it's Android-only;
    // the iOS module provides a harmless no-op.
    AsyncFunction("ensureCustomSoundChannel") {
        channelId: String, soundPath: String, channelName: String ->
      if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return@AsyncFunction
      val context = appContext.reactContext?.applicationContext
        ?: throw IllegalStateException("CalFfi: no Android context for the sound channel")
      val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      val uri = FileProvider.getUriForFile(
        context,
        "${context.packageName}.remindersounds",
        File(soundPath),
      )
      // The system-UI process plays the channel sound — grant it read access to
      // our not-exported provider's URI. Re-issued on EVERY call (the scheduler
      // invokes this on each reschedule): the grant is transient + not guaranteed
      // to survive a reboot, so re-granting before the create-once guard keeps a
      // persisted channel's sound readable after a restart. Cheap + idempotent.
      context.grantUriPermission(
        "com.android.systemui",
        uri,
        Intent.FLAG_GRANT_READ_URI_PERMISSION,
      )
      // The channel's sound can't change after creation; the per-sound id means
      // we only need to create the channel once.
      if (nm.getNotificationChannel(channelId) != null) return@AsyncFunction
      val attrs = AudioAttributes.Builder()
        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
        .setUsage(AudioAttributes.USAGE_NOTIFICATION)
        .build()
      val channel =
        NotificationChannel(channelId, channelName, NotificationManager.IMPORTANCE_HIGH)
      channel.setSound(uri, attrs)
      nm.createNotificationChannel(channel)
    }
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.contactFunctions() {
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
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.meetingFunctions() {
    // ─── Meetings (network: they talk to the provider) ───────────────────────

    AsyncFunction("attachMeetingJson") { requestJson: String ->
      host.attachMeetingJson(requestJson)
    }.runOnQueue(slowScope)

    AsyncFunction("detachMeetingJson") { requestJson: String ->
      host.detachMeetingJson(requestJson)
    }.runOnQueue(slowScope)

    // Network: it asks the provider about the meeting.
    AsyncFunction("inspectEventMeetingJson") { requestJson: String ->
      host.inspectEventMeetingJson(requestJson)
    }.runOnQueue(slowScope)

    AsyncFunction("adoptMeetingJson") { requestJson: String ->
      host.adoptMeetingJson(requestJson)
    }

    AsyncFunction("eventMeetingJson") { eventId: String, calendarId: String? ->
      host.eventMeetingJson(eventId, calendarId)
    }

    AsyncFunction("listAdapterKindsJson") {
      host.listAdapterKindsJson()
    }

    AsyncFunction("accountFormSpecJson") { adapterKind: String, lang: String? ->
      host.accountFormSpecJson(adapterKind, lang)
    }

    // Pure (no network): builds the consent URL + PKCE verifier + state.
    AsyncFunction("beginAccountOauthJson") { adapterKind: String, valuesJson: String ->
      host.beginAccountOauthJson(adapterKind, valuesJson)
    }

    // Network: the token exchange, then the row + keychain + registration.
    AsyncFunction("connectAccountJson") { requestJson: String ->
      host.connectAccountJson(requestJson)
    }.runOnQueue(slowScope)

    // Re-sign-in for an EXISTING account. Both halves take only the account id:
    // everything else — which client, which redirect, whether there is a client
    // secret at all — the host reads off the account and its adapter's schema.
    AsyncFunction("beginAccountReconnectJson") { accountId: String ->
      host.beginAccountReconnectJson(accountId)
    }

    AsyncFunction("completeAccountReconnectJson") { accountId: String, requestJson: String ->
      host.completeAccountReconnectJson(accountId, requestJson)
    }.runOnQueue(slowScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.onboardingFunctions() {
    // ─── Onboarding: preview + join an existing dataset (§19.11) ──────────────
    // previewSyncTargetJson reads the target's meta.json WITHOUT committing →
    // {kind: empty | existing, …}. acceptRemoteDatasetJson joins an existing
    // dataset (deriving the E2E key from the passphrase when it's encrypted).

    AsyncFunction("previewSyncTargetJson") { configJson: String ->
      coded { host.previewSyncTargetJson(configJson) }
    }.runOnQueue(slowScope)

    AsyncFunction("previewSyncTargetValuesJson") { requestJson: String ->
      coded { host.previewSyncTargetValuesJson(requestJson) }
    }.runOnQueue(slowScope)

    AsyncFunction("acceptRemoteDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      coded { host.acceptRemoteDatasetJson(configJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    // adoptLocalDatasetJson initialises a FRESH dataset (the unified Connect
    // button's empty-target path), optionally enabling E2E at creation.
    AsyncFunction("adoptLocalDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      coded { host.adoptLocalDatasetJson(configJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    // The same two answers, asked with the shared schema form's own values, and
    // committing to an ACCOUNT ROW rather than to device-local preferences.
    AsyncFunction("acceptRemoteDatasetValuesJson") { requestJson: String, deviceName: String?, passphrase: String? ->
      coded { host.acceptRemoteDatasetValuesJson(requestJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    AsyncFunction("adoptLocalDatasetValuesJson") { requestJson: String, deviceName: String?, passphrase: String? ->
      coded { host.adoptLocalDatasetValuesJson(requestJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    AsyncFunction("resumeStaleDeviceJson") {
      coded { host.resumeStaleDeviceJson() }
    }.runOnQueue(slowScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.sftpFunctions() {
    // ─── SFTP host-key trust (§19.5 TOFU) ─────────────────────────────────────
    // previewSftpHostKeyJson probes the server's fingerprint (network) + compares
    // it to the device pin store → {host_port, fingerprint, status}; trust/forget/
    // pinned manage the pin (no network). The JS layer shows the trust dialog.

    AsyncFunction("previewSftpHostKeyJson") { argsJson: String ->
      coded { host.previewSftpHostKeyJson(argsJson) }
    }.runOnQueue(slowScope)

    // The same probe for an account the user is about to sync through — the
    // repair for a selectSyncAccount that refused an unconfirmed host key.
    // Returns the JSON `null` (no network) when the adapter pins no host key.
    AsyncFunction("previewSyncAccountHostKeyJson") { accountId: String ->
      coded { host.previewSyncAccountHostKeyJson(accountId) }
    }.runOnQueue(slowScope)

    // Read-only, and no network: what this device already confirmed.
    AsyncFunction("syncAccountHostKeyPinJson") { accountId: String ->
      coded { host.syncAccountHostKeyPinJson(accountId) }
    }

    AsyncFunction("trustSftpHostKey") { hostPort: String, fingerprint: String ->
      coded { host.trustSftpHostKey(hostPort, fingerprint) }
    }

    AsyncFunction("forgetSftpHostKey") { hostPort: String ->
      host.forgetSftpHostKey(hostPort)
    }

    AsyncFunction("pinnedSftpHostKey") { hostPort: String ->
      host.pinnedSftpHostKey(hostPort)
    }

    // §19 device registry: this device's own name, and the list of everyone the
    // dataset still counts as a participant.
    AsyncFunction("syncDeviceNameJson") {
      coded { host.syncDeviceNameJson() }
    }

    AsyncFunction("setSyncDeviceName") { name: String ->
      coded { host.setSyncDeviceName(name) }
    }

    AsyncFunction("listSyncDevicesJson") {
      coded { host.listSyncDevicesJson() }
    }

    AsyncFunction("forgetSyncDevice") { deviceId: String ->
      coded { host.forgetSyncDevice(deviceId) }
    }
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.widgetFunctions() {
    // ── Widgets ──
    // The same snapshot document iOS uses, in the app's own internal storage —
    // an Android widget runs in this very process under the same uid, so there
    // is no App Group to cross and no container to resolve.
    // One line into the app's own rolling log — the twin of the iOS registration.
    AsyncFunction("logLine") { level: String, message: String ->
      host.logLine(level, message)
    }

    AsyncFunction("writeWidgetSnapshot") { json: String ->
      val context = appContext.reactContext?.applicationContext
      if (context != null) {
        WidgetStore.writeSnapshot(context, json)
        // Redraw every placed instance. Without this the launcher would keep
        // showing the previous timeline until the system next asked on its own.
        slowScope.launch {
          try {
            AperioWidget().updateAll(context)
          } catch (_: Throwable) {
            // No widget placed, or the launcher declined — neither is a reason
            // to fail a caller that is only keeping a convenience current.
          }
        }
      }
    }

    AsyncFunction("pendingWidgetActionsJson") { ->
      appContext.reactContext?.applicationContext?.let { WidgetStore.pendingJson(it) } ?: "[]"
    }

    AsyncFunction("clearWidgetAction") { id: String ->
      appContext.reactContext?.applicationContext?.let { WidgetStore.clearAction(it, id) }
    }
  }
}
