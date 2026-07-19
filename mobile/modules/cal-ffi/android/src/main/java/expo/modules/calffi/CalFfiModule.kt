package expo.modules.calffi

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.os.Build
import android.util.Log
import androidx.core.content.FileProvider
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File
import java.util.concurrent.Executors
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import uniffi.cal_ffi.Host
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

  /// Adapts the UniFFI `ContactSyncObserverBridge` callback to an Expo event.
  /// Fired on a background thread when a contact-sync pass finishes; `sendEvent`
  /// marshals to JS. Mirrors `JsCacheObserver`.
  private inner class JsContactSyncObserver : uniffi.cal_ffi.ContactSyncObserverBridge {
    override fun contactsSynced(payloadJson: String) {
      this@CalFfiModule.sendEvent("onContactsSynced", mapOf("payload" to payloadJson))
    }
  }

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

    // ─── Sync (full desktop peer: same engine, statically-embedded adapters) ──
    // configure sets the active sync target; sync_now runs a round + returns the
    // report; a thrown StoreException rejects the JS promise.

    AsyncFunction("configureSyncAdapterJson") { configJson: String ->
      host.configureSyncAdapterJson(configJson)
    }.runOnQueue(slowScope)

    AsyncFunction("syncStatusJson") {
      host.syncStatusJson()
    }

    AsyncFunction("syncNowJson") { trigger: String ->
      host.syncNowJson(trigger)
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
      host.compactNowJson()
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
      host.syncContactsNow(includeReadOnly)
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
    }.runOnQueue(slowScope)

    AsyncFunction("completeOauthReconnectJson") { pluginId: String, accountId: String, requestJson: String ->
      host.completeOauthReconnectJson(pluginId, accountId, requestJson)
    }.runOnQueue(slowScope)

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
      host.enableSyncEncryptionJson(passphrase)
    }.runOnQueue(slowScope)

    // disableSyncEncryptionJson turns E2E OFF: rewrites every log + snapshot as
    // plaintext, flips the meta, drops the device key (other devices re-onboard).
    AsyncFunction("disableSyncEncryptionJson") { passphrase: String ->
      host.disableSyncEncryptionJson(passphrase)
    }.runOnQueue(slowScope)

    // changeSyncPassphraseJson rotates the E2E passphrase (re-wraps the same
    // data key; existing devices keep working, future joins need the new one).
    AsyncFunction("changeSyncPassphraseJson") { oldPassphrase: String, newPassphrase: String ->
      host.changeSyncPassphraseJson(oldPassphrase, newPassphrase)
    }.runOnQueue(slowScope)

    // adoptRemoteEncryptionJson: a peer turned E2E on while this device synced
    // plaintext; derive the key from the passphrase + swap to an encrypting
    // adapter so the next round (which had failed with encryption_required) works.
    AsyncFunction("adoptRemoteEncryptionJson") { passphrase: String ->
      host.adoptRemoteEncryptionJson(passphrase)
    }.runOnQueue(slowScope)

    // ─── Onboarding: preview + join an existing dataset (§19.11) ──────────────
    // previewSyncTargetJson reads the target's meta.json WITHOUT committing →
    // {kind: empty | existing, …}. acceptRemoteDatasetJson joins an existing
    // dataset (deriving the E2E key from the passphrase when it's encrypted).

    AsyncFunction("previewSyncTargetJson") { configJson: String ->
      host.previewSyncTargetJson(configJson)
    }.runOnQueue(slowScope)

    AsyncFunction("acceptRemoteDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      host.acceptRemoteDatasetJson(configJson, deviceName, passphrase)
    }.runOnQueue(slowScope)

    // adoptLocalDatasetJson initialises a FRESH dataset (the unified Connect
    // button's empty-target path), optionally enabling E2E at creation.
    AsyncFunction("adoptLocalDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      host.adoptLocalDatasetJson(configJson, deviceName, passphrase)
    }.runOnQueue(slowScope)

    AsyncFunction("resumeStaleDeviceJson") {
      host.resumeStaleDeviceJson()
    }.runOnQueue(slowScope)

    // ─── SFTP host-key trust (§19.5 TOFU) ─────────────────────────────────────
    // previewSftpHostKeyJson probes the server's fingerprint (network) + compares
    // it to the device pin store → {host_port, fingerprint, status}; trust/forget/
    // pinned manage the pin (no network). The JS layer shows the trust dialog.

    AsyncFunction("previewSftpHostKeyJson") { argsJson: String ->
      host.previewSftpHostKeyJson(argsJson)
    }.runOnQueue(slowScope)

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
