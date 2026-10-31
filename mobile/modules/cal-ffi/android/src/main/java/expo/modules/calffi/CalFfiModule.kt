package expo.modules.calffi

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import android.util.Log
import androidx.core.content.FileProvider
// An extension function on GlanceAppWidget — see AperioWidget.kt.
import androidx.glance.appwidget.updateAll
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.modules.ModuleDefinitionBuilder
import java.io.File
import org.json.JSONObject
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
import uniffi.cal_ffi.compareNames as uniffiCompareNames
import uniffi.cal_ffi.compareTitles as uniffiCompareTitles
import uniffi.cal_ffi.detectConference as uniffiDetectConference
import uniffi.cal_ffi.findGroupSuggestions as uniffiFindGroupSuggestions
import uniffi.cal_ffi.carryOntoFields as uniffiCarryOntoFields
import uniffi.cal_ffi.groupTasks as uniffiGroupTasks
import uniffi.cal_ffi.tasksOnDays as uniffiTasksOnDays
import uniffi.cal_ffi.backlogWeeks as uniffiBacklogWeeks
import uniffi.cal_ffi.splitDeadlinesByWeek as uniffiSplitDeadlinesByWeek
import uniffi.cal_ffi.taskI18nKeys as uniffiTaskI18nKeys
import uniffi.cal_ffi.subtaskProgress as uniffiSubtaskProgress
import uniffi.cal_ffi.planStatusCascade as uniffiPlanStatusCascade
import uniffi.cal_ffi.planAncestorRecompute as uniffiPlanAncestorRecompute
import uniffi.cal_ffi.autoDateOnStart as uniffiAutoDateOnStart
import uniffi.cal_ffi.expandTaskOccurrences as uniffiExpandTaskOccurrences
import uniffi.cal_ffi.nextTaskOccurrence as uniffiNextTaskOccurrence
import uniffi.cal_ffi.occurrenceMoveTarget as uniffiOccurrenceMoveTarget
import uniffi.cal_ffi.selfAssignOnStatusChange as uniffiSelfAssignOnStatusChange
import uniffi.cal_ffi.taskAssignmentMode as uniffiTaskAssignmentMode
import uniffi.cal_ffi.clampAssignees as uniffiClampAssignees
import uniffi.cal_ffi.signatureIn as uniffiSignatureIn
import uniffi.cal_ffi.stripSignature as uniffiStripSignature
import uniffi.cal_ffi.applySignature as uniffiApplySignature
import uniffi.cal_ffi.dayStart as uniffiDayStart
import uniffi.cal_ffi.taskSettings as uniffiTaskSettings
import uniffi.cal_ffi.recurrenceSummary as uniffiRecurrenceSummary
import uniffi.cal_ffi.seriesShift as uniffiSeriesShift
import uniffi.cal_ffi.tailExceptions as uniffiTailExceptions
import uniffi.cal_ffi.collapseEventGroups as uniffiCollapseEventGroups
import uniffi.cal_ffi.futureCarryFields as uniffiFutureCarryFields
import uniffi.cal_ffi.findMeetingLinkPairs as uniffiFindMeetingLinkPairs
import uniffi.cal_ffi.normalizeJoinUrl as uniffiNormalizeJoinUrl
import uniffi.cal_ffi.seriesClockZone as uniffiSeriesClockZone
import uniffi.cal_ffi.canonicalZone as uniffiCanonicalZone
import uniffi.cal_ffi.expansionClock as uniffiExpansionClock
import uniffi.cal_ffi.zoneLabels as uniffiZoneLabels
import uniffi.cal_ffi.zoneSearch as uniffiZoneSearch
import uniffi.cal_ffi.zoneChoice as uniffiZoneChoice
import uniffi.cal_ffi.zoneOffsets as uniffiZoneOffsets
import uniffi.cal_ffi.occurrenceCarryFields as uniffiOccurrenceCarryFields
import uniffi.cal_ffi.planCarry as uniffiPlanCarry
import uniffi.cal_ffi.isImportantPriority as uniffiIsImportantPriority
import uniffi.cal_ffi.normalPriority as uniffiNormalPriority
import uniffi.cal_ffi.priorityRank as uniffiPriorityRank
import uniffi.cal_ffi.parseAttendee as uniffiParseAttendee
import uniffi.cal_ffi.suggestGroupMate as uniffiSuggestGroupMate
import uniffi.cal_ffi.withoutDuplicateMeetings as uniffiWithoutDuplicateMeetings

/** One byte past the most an import reads (`host_core::file_import::
 *  MAX_IMPORT_BYTES`, 25 MiB): enough for the read to call a file too large. */
private const val INCOMING_CAP: Long = 25L * 1024 * 1024 + 1

/** Where the launch intent last taken is remembered, across processes. */
private const val INCOMING_PREFS = "aperio_incoming_file"
private const val TAKEN_LAUNCH = "taken_launch"

class CalFfiModule : Module() {
  // Expo runs EVERY AsyncFunction body on ONE single-threaded HandlerThread
  // dispatcher ("expo.modules.AsyncFunctionQueue"), so a seconds-long sync
  // round makes every read call queue behind it — on app open the views then
  // sit on "Loading…" until the launch sync finishes, even with a warm SWR
  // cache. The long NETWORK-bound operations therefore run on their own
  // single-thread scope: sync rounds stay mutually exclusive among themselves,
  // while the reads, writes and lookups on the scopes below flow past them.
  // (The Rust Host is Send+Sync — uniffi's contract — and its SQLite access is
  // mutex-guarded, so every dispatcher may safely call into it concurrently.)
  // What is left on the default queue is short: preferences, logs, sounds,
  // the widget, account settings, and the calls that only start a refresh.
  // Mirrors the iOS module's `slowQueue`; each scope here has its twin there,
  // and check-ffi-bridges.mjs holds every function to the same one on both
  // platforms.
  // The dispatcher is kept separately so OnDestroy can close it — expo-modules
  // only cancels its OWN queues on teardown, so without this every React-host
  // re-creation (dev reload) would leak one "calffi.slow" thread.
  private val slowDispatcher =
    Executors.newSingleThreadExecutor { r -> Thread(r, "calffi.slow") }
      .asCoroutineDispatcher()
  private val slowScope = CoroutineScope(
    slowDispatcher + SupervisorJob() + CoroutineName("calffi.slow"),
  )

  // Reads of what the host already holds, the cache and the local store, run
  // side by side: paging a view during an account refresh used to wait behind
  // whatever else was queued, and the new days stayed empty until it finished.
  // Four at a time, as many as the host's read pool has WAL connections. The
  // reads never ask a provider: a container that is not cached serves what it
  // has and starts a refresh in the background. Mirrors `readQueue`.
  private val readDispatcher = ownThreads("calffi.read", 4)
  private val readScope = CoroutineScope(
    readDispatcher + SupervisorJob() + CoroutineName("calffi.read"),
  )

  // Every change to calendars, events, tasks, contacts and what hangs on
  // them (colours, groups, markers, local reminders), one after another in
  // the order JavaScript sent them: a colour set right after a save must not
  // land before it, nor a binding before the id it names. Apart from the
  // reads, so a slow provider write holds up no view. Mirrors `writeQueue`.
  private val writeDispatcher = ownThreads("calffi.write")
  private val writeScope = CoroutineScope(
    writeDispatcher + SupervisorJob() + CoroutineName("calffi.write"),
  )

  // Reads that ask a provider live (contact search and photos, list members
  // and shares, the signed-in user, the sync devices). Seconds on a slow
  // network, so they wait neither for the cache reads nor in front of them.
  // Mirrors `lookupQueue`.
  private val lookupDispatcher = ownThreads("calffi.lookup")
  private val lookupScope = CoroutineScope(
    lookupDispatcher + SupervisorJob() + CoroutineName("calffi.lookup"),
  )

  // The reminder scan, `upcomingRemindersJson`, reads every account live,
  // three to five seconds. On the default queue a check-off waited behind it
  // (TODO B13, 218); on `slowScope` it would wait for sync rounds and they for
  // it. Mirrors `remindersQueue`.
  private val remindersDispatcher = ownThreads("calffi.reminders")
  private val remindersScope = CoroutineScope(
    remindersDispatcher + SupervisorJob() + CoroutineName("calffi.reminders"),
  )

  /** A dispatcher on `count` threads of its own, named for the logs; OnDestroy closes it. */
  private fun ownThreads(name: String, count: Int = 1) =
    Executors.newFixedThreadPool(count) { r -> Thread(r, name) }.asCoroutineDispatcher()

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
   * Re-throw any failure of the Rust host with the desktop's code and its own
   * message (decision 273; TODO B9).
   *
   * Expo turns an unmapped exception into a JS error whose message is
   * Kotlin's `toString()` — "detail=provider-busy: http-429" — under a
   * generic code, so the shared translator matched nothing: a refusal or a
   * leftover token was read out in English, and a sync failure showed the
   * engine's text. `CodedException` is the shape Expo surfaces as
   * `error.code`. The codes are the desktop's (`CommandError`), so one
   * translator serves both surfaces; a sync failure keeps the engine's own.
   *
   * NOT inline: every registration that calls the host wraps its body in it
   * (check-ffi-bridges.mjs fails one that does not), and an inlined try/catch
   * in each would grow the module's methods toward the JVM's 64 KB cap.
   */
  private fun <T> hostCoded(block: () -> T): T =
    try {
      block()
    } catch (e: StoreException) {
      throw when (e) {
        is StoreException.Sync -> CodedException(e.code, e.detail, e)
        is StoreException.Auth -> CodedException("auth", e.detail, e)
        is StoreException.Forbidden -> CodedException("forbidden", e.detail, e)
        is StoreException.NotFound -> CodedException("not_found", "not found", e)
        is StoreException.Conflict -> CodedException("conflict", e.detail, e)
        is StoreException.Network -> CodedException("network", e.detail, e)
        is StoreException.Protocol -> CodedException("protocol", e.detail, e)
        is StoreException.InvalidField ->
          CodedException("invalid_input", "${e.field}: ${e.detail}", e)
        is StoreException.Unsupported -> CodedException("unsupported", e.detail, e)
        is StoreException.Open -> CodedException("internal", e.detail, e)
        is StoreException.Storage -> CodedException("internal", e.detail, e)
      }
    }

  /**
   * [hostCoded] for a grouping request, whose one conflict can only mean that
   * both named events are already in different groups: a sentence of its own
   * ("take one of them out first").
   */
  private fun <T> groupCoded(block: () -> T): T =
    hostCoded {
      try {
        block()
      } catch (e: StoreException.Conflict) {
        throw CodedException("event_group_conflict", e.detail, e)
      }
    }

  /**
   * Re-throw a file that could not be read for an import with the desktop's
   * code (DESIGN-file-import.md, I5): `unsupported` (no plugin reads it),
   * `invalid_input` (unreadable, or too large: the detail then starts with
   * `import-too-large`), `internal` (not opened). The shared
   * `importFailureSentence` says why from these, in the reader's language.
   * Every other error keeps its code through [hostCoded] (decision 273).
   */
  private fun <T> importCoded(block: () -> T): T =
    hostCoded {
      try {
        block()
      } catch (e: StoreException.InvalidField) {
        // The detail alone: it starts with `import-too-large` when it is that.
        throw CodedException("invalid_input", e.detail, e)
      }
    }

  // Split across several `ModuleDefinitionBuilder` extensions rather than one
  // lambda. The JVM caps a single method's bytecode at 64 KB, and every
  // registration is inlined into the method it sits in: 140 went past it once
  // ("Method too large: CalFfiModule.definition()"). The first split left 66
  // here, and new registrations kept landing here rather than in a group until
  // 130 went past it again. So a new registration goes into the group it
  // belongs to, not into definition(). Each extension compiles to its own
  // method, so the ceiling applies per group; the FFI bridge check
  // (mobile/scripts/check-ffi-bridges.mjs) fails a method holding more than 70
  // before a release build finds out.
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

    // Release the module's own threads when it is torn down (dev reload /
    // React-host restart) — expo-modules cancels only its own queues.
    OnDestroy {
      slowScope.cancel()
      slowDispatcher.close()
      readScope.cancel()
      readDispatcher.close()
      writeScope.cancel()
      writeDispatcher.close()
      lookupScope.cancel()
      lookupDispatcher.close()
      remindersScope.cancel()
      remindersDispatcher.close()
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

    coreRuleFunctions()

    taskFunctions()

    accountFunctions()

    calendarFunctions()

    fileImportFunctions()

    syncFunctions()

    // ─── Reminders ────────────────────────────────────────────────────────────
    // Upcoming reminder triggers (local + external) within a horizon, for the JS
    // layer to schedule as expo-notifications. `horizonMinutes` arrives as a JS
    // Number → widen the Int to the Rust u32.

    AsyncFunction("upcomingRemindersJson") { horizonMinutes: Int ->
      hostCoded {
        host.upcomingRemindersJson(horizonMinutes.toUInt())
      }
    }.runOnQueue(remindersScope)

    soundFunctions()

    preferenceFunctions()

    contactFunctions()

    collaborationFunctions()

    // ─── Schema-driven accounts ──────────────────────────────────────────────
    // The generic connect path: the adapter declares its form in its
    // plugin.json and the host executes the declaration, so adding an adapter
    // adds no code here either.

    meetingFunctions()

    oauthFunctions()

    onboardingFunctions()

    sftpFunctions()

    widgetFunctions()
  }

  /**
   * Lifted out of `definition()` — see the note there. The core's exported free
   * functions, the rules that need no Host: text ordering, priority, conference
   * detection, event groups, zones, the carry, task grouping and task
   * recurrence, the status cascade, assignment, signatures, the day start, task
   * settings, series shifts and recurrence words. `parseAttendee`, the first of
   * them, stays in `definition()`.
   */
  private fun ModuleDefinitionBuilder.coreRuleFunctions() {
    // ─── Text ordering (synchronous, and that is the point) ───
    // The tiebreaker every list in this app ends in, from `cal_core::collation`
    // via the Kotlin bindings. `Function`, not `AsyncFunction`: the callers are
    // `Array.prototype.sort` comparators, which cannot await. Same reason the
    // desktop reaches these through WebAssembly — see DESIGN §4.3 for the door,
    // §4.4 for the rule that goes through it.
    //
    // `languageTag` is the language the USER chose in Aperio. Every
    // `localeCompare` this replaces passed `undefined` and so followed the
    // DEVICE, which meant German text ordered by English rules on an English
    // phone.
    Function("compareNames") { a: String, b: String, languageTag: String ->
      uniffiCompareNames(a, b, languageTag)
    }

    Function("compareTitles") { a: String, b: String, languageTag: String ->
      uniffiCompareTitles(a, b, languageTag)
    }

    // ─── Task priority (synchronous, and that is the point) ───
    // The ranking every task list sorts by, from `cal_core::task_priority`.
    // `Function`, not `AsyncFunction`: the callers are `Array.prototype.sort`
    // comparators, which cannot await.
    //
    // Until these existed the rule was reachable from the DESKTOP only — it
    // lived in `crates/cal-core-wasm`, the desktop's own binding — so this app
    // ran the TypeScript copy while the desktop ran Rust. Two devices showing
    // one task list have to put the same task first.
    Function("priorityRank") { priority: String, scale: String ->
      uniffiPriorityRank(priority, scale)
    }

    Function("isImportantPriority") { priority: String ->
      uniffiIsImportantPriority(priority)
    }

    Function("normalPriority") { previous: String ->
      uniffiNormalPriority(previous)
    }

    // ─── Conference detection ───
    // Finding the online meeting in an event, from `cal_core::conferencing`.
    // JSON in, JSON out — the same shape the desktop's WebAssembly door uses.
    //
    // This replaces `shared/conferencing.ts`, a second implementation of the
    // same rule that ran here in production with nothing pinning the two
    // together.
    Function("detectConference") { sourcesJson: String ->
      uniffiDetectConference(sourcesJson)
    }

    // Recognising a copy — "these two rows look like one appointment".
    //
    // `Function`, not `AsyncFunction`, and that is the whole point: both
    // callers ask during a render (`useMemo` in the suggestion notice and in
    // the grouping modal), where nothing can await. This replaces
    // `shared/groupSuggestions.ts` and `shared/suggestGroupMate.ts`, two
    // second implementations of rules the core now holds once.
    Function("findGroupSuggestions") { inputJson: String ->
      uniffiFindGroupSuggestions(inputJson)
    }

    Function("suggestGroupMate") { inputJson: String ->
      uniffiSuggestGroupMate(inputJson)
    }

    // Hiding a provider-side meeting that already has a calendar entry. The
    // whole window crosses at once; the rule this replaces asked the
    // detection door twice per row.
    Function("withoutDuplicateMeetings") { eventsJson: String ->
      uniffiWithoutDuplicateMeetings(eventsJson)
    }

    // Pairing a meeting with its appointment, and the URL fold the identity
    // rests on. Synchronous — the caller decides during a render pass.
    Function("findMeetingLinkPairs") { inputJson: String ->
      uniffiFindMeetingLinkPairs(inputJson)
    }

    Function("normalizeJoinUrl") { url: String ->
      uniffiNormalizeJoinUrl(url)
    }

    // The clock a series repeats on: the stored name, or "" for UTC.
    // Synchronous — every expansion of a series asks while it renders.
    Function("seriesClockZone") { tzid: String ->
      uniffiSeriesClockZone(tzid)
    }

    Function("canonicalZone") { name: String ->
      uniffiCanonicalZone(name)
    }

    // Which clock a series' rule is read on: an all-day series repeats on the
    // device's calendar days (48a). Synchronous, for the same reason.
    Function("expansionClock") { allDay: Boolean, tzid: String ->
      uniffiExpansionClock(allDay, tzid)
    }

    // The world zone list: its names, the search, where a stored or device
    // zone stands, and the offsets. Synchronous like every rule door; the
    // offsets cost well under a millisecond on the desktop's native build.
    Function("zoneLabels") {
      uniffiZoneLabels()
    }

    Function("zoneSearch") { inputJson: String ->
      uniffiZoneSearch(inputJson)
    }

    Function("zoneChoice") { inputJson: String ->
      uniffiZoneChoice(inputJson)
    }

    Function("zoneOffsets") { inputJson: String ->
      uniffiZoneOffsets(inputJson)
    }

    // Folding a group into one row — what a day actually looks like.
    Function("collapseEventGroups") { inputJson: String ->
      uniffiCollapseEventGroups(inputJson)
    }

    // Carrying a change to the other copies. Synchronous — the editor decides
    // whether to ask the question while it is saving.
    Function("planCarry") { inputJson: String ->
      uniffiPlanCarry(inputJson)
    }

    Function("occurrenceCarryFields") { inputJson: String ->
      uniffiOccurrenceCarryFields(inputJson)
    }

    Function("futureCarryFields") { inputJson: String ->
      uniffiFutureCarryFields(inputJson)
    }

    Function("carryOntoFields") { inputJson: String ->
      uniffiCarryOntoFields(inputJson)
    }

    // Grouping the task view. Asked while rendering.
    Function("groupTasks") { inputJson: String ->
      uniffiGroupTasks(inputJson)
    }

    // The calendar-day task rules. Asked while rendering.
    Function("tasksOnDays") { inputJson: String ->
      uniffiTasksOnDays(inputJson)
    }

    Function("backlogWeeks") { inputJson: String ->
      uniffiBacklogWeeks(inputJson)
    }

    Function("splitDeadlinesByWeek") { inputJson: String ->
      uniffiSplitDeadlinesByWeek(inputJson)
    }

    // The task vocabulary's i18n keys (read once at install) and the subtask
    // progress for a whole list.
    Function("taskI18nKeys") {
      uniffiTaskI18nKeys()
    }

    Function("subtaskProgress") { inputJson: String ->
      uniffiSubtaskProgress(inputJson)
    }

    // The parent/subtask status coupling: the writes a status change plans,
    // the ancestors re-derived after a subtask came or went, the auto-date.
    Function("planStatusCascade") { inputJson: String ->
      uniffiPlanStatusCascade(inputJson)
    }

    Function("planAncestorRecompute") { inputJson: String ->
      uniffiPlanAncestorRecompute(inputJson)
    }

    Function("autoDateOnStart") { inputJson: String ->
      uniffiAutoDateOnStart(inputJson)
    }

    // The recurring-task projection: occurrences in a window, one step of
    // the walk, and what a move can be where the source owns the date.
    Function("expandTaskOccurrences") { inputJson: String ->
      uniffiExpandTaskOccurrences(inputJson)
    }

    Function("nextTaskOccurrence") { inputJson: String ->
      uniffiNextTaskOccurrence(inputJson)
    }

    Function("occurrenceMoveTarget") { inputJson: String ->
      uniffiOccurrenceMoveTarget(inputJson)
    }

    // The assignment rules: who holds a task after a status change, how many
    // a list can hold, what a list of assignees is trimmed to.
    Function("selfAssignOnStatusChange") { inputJson: String ->
      uniffiSelfAssignOnStatusChange(inputJson)
    }

    Function("taskAssignmentMode") { inputJson: String ->
      uniffiTaskAssignmentMode(inputJson)
    }

    Function("clampAssignees") { inputJson: String ->
      uniffiClampAssignees(inputJson)
    }

    // The signature block of a description: read, strip, apply.
    Function("signatureIn") { inputJson: String ->
      uniffiSignatureIn(inputJson)
    }

    Function("stripSignature") { inputJson: String ->
      uniffiStripSignature(inputJson)
    }

    Function("applySignature") { inputJson: String ->
      uniffiApplySignature(inputJson)
    }

    // The day-start rules: one question, one answer.
    Function("dayStart") { inputJson: String ->
      uniffiDayStart(inputJson)
    }

    // The task settings: reading, effective settings per list, what a change stores.
    Function("taskSettings") { inputJson: String ->
      uniffiTaskSettings(inputJson)
    }

    // Shifting a recurring series by whole days: one question, one answer.
    Function("seriesShift") { inputJson: String ->
      uniffiSeriesShift(inputJson)
    }

    // The deleted occurrences a series keeps from a cut on: one question, one answer.
    Function("tailExceptions") { inputJson: String ->
      uniffiTailExceptions(inputJson)
    }

    // A repeat rule in words: keys and values the surface renders.
    Function("recurrenceSummary") { inputJson: String ->
      uniffiRecurrenceSummary(inputJson)
    }
  }

  /** Lifted out of `definition()` — see the note there. Tasks, lists and sections. */
  private fun ModuleDefinitionBuilder.taskFunctions() {
    // ─── Tasks / lists / sections (JSON bridge, sync-logged) ─────────────────
    // The full task / list / section domain crosses as a JSON string in the
    // cal_core serde shape — identical to the desktop's Tauri payloads — so
    // this layer is a trivial passthrough: the mobile api-client parses the
    // JSON into the shared @aperio/shared types, keeping the marshalling in one
    // place. Each Host mutation appends the matching SyncEvent. All store ops
    // are `AsyncFunction`s: Expo dispatches them off the JS thread, and a thrown
    // StoreException rejects the JS promise.

    AsyncFunction("taskListsJson") {
      hostCoded {
        host.taskListsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("createTaskListJson") { name: String ->
      hostCoded {
        host.createTaskListJson(name)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("reparentTaskList") { id: String, parentId: String? ->
      hostCoded {
        host.reparentTaskList(id, parentId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteTaskList") { id: String ->
      hostCoded {
        host.deleteTaskList(id)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("tasksJson") { listId: String ->
      hostCoded {
        host.tasksJson(listId)
      }
    }.runOnQueue(readScope)

    AsyncFunction("taskJson") { id: String ->
      hostCoded {
        host.taskJson(id)
      }
    }.runOnQueue(readScope)

    AsyncFunction("createTaskJson") { listId: String, newTaskJson: String ->
      hostCoded { host.createTaskJson(listId, newTaskJson) }
    }.runOnQueue(writeScope)

    AsyncFunction("updateTaskJson") { taskJson: String, previousListId: String? ->
      hostCoded { host.updateTaskJson(taskJson, previousListId) }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteTask") { taskId: String, listId: String? ->
      hostCoded { host.deleteTask(taskId, listId) }
    }.runOnQueue(writeScope)

    AsyncFunction("sectionsJson") { listId: String ->
      hostCoded {
        host.sectionsJson(listId)
      }
    }.runOnQueue(readScope)

    // `position` arrives from JS as a Number; the Rust signature takes a u32
    // (Kotlin UInt), which Expo can't coerce a JS Number to directly, so take
    // an Int and widen it here.
    AsyncFunction("createSectionJson") { listId: String, name: String, position: Int, colorLabel: String? ->
      hostCoded {
        host.createSectionJson(listId, name, position.toUInt(), colorLabel)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("updateSectionJson") { sectionJson: String ->
      hostCoded {
        host.updateSectionJson(sectionJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteSection") { id: String, listId: String? ->
      hostCoded {
        host.deleteSection(id, listId)
      }
    }.runOnQueue(writeScope)
  }

  /** Lifted out of `definition()` — see the note there. Accounts: the full engine, external adapters and secrets. */
  private fun ModuleDefinitionBuilder.accountFunctions() {
    // ─── Accounts (the full engine: external adapters + secrets) ─────────────
    // JSON passthrough in the cal_core/desktop wire shape, same convention as
    // the task bridge. create_account_json persists the row, stores the secret
    // via the keychain bridge, and registers the adapter; a thrown
    // StoreException rejects the JS promise.

    AsyncFunction("accountsJson") { lang: String? ->
      hostCoded {
        host.accountsJson(lang)
      }
    }.runOnQueue(readScope)

    AsyncFunction("createAccountJson") { requestJson: String ->
      hostCoded {
        host.createAccountJson(requestJson)
      }
    }

    AsyncFunction("testAccountJson") { requestJson: String ->
      hostCoded {
        host.testAccountJson(requestJson)
      }
    }.runOnQueue(slowScope)

    AsyncFunction("testAccountValuesJson") { requestJson: String ->
      hostCoded {
        host.testAccountValuesJson(requestJson)
      }
    }.runOnQueue(slowScope)

    AsyncFunction("runAccountActionJson") { requestJson: String ->
      hostCoded {
        host.runAccountActionJson(requestJson)
      }
    }.runOnQueue(slowScope)

    AsyncFunction("deleteAccount") { accountId: String ->
      hostCoded {
        host.deleteAccount(accountId)
      }
    }

    AsyncFunction("renameAccountJson") { id: String, newName: String ->
      hostCoded {
        host.renameAccountJson(id, newName)
      }
    }

    // Uniform JS surface with iOS. Nothing on Android calls it: the RN layer
    // requests the runtime permission with PermissionsAndroid itself, and notes
    // each request through `noteDeviceCalendarAsked`. If called, the bridge only
    // reports whether READ_CALENDAR is granted.
    AsyncFunction("requestDeviceCalendarAccess") { events: Boolean, reminders: Boolean ->
      hostCoded {
        host.requestDeviceCalendarAccess(events, reminders)
      }
    }

    // What the OS allows for the device calendars, and whether to ask now
    // (decisions 166, 172). Asks nobody. "Not asked" on Android is Aperio's own
    // record (see `noteDeviceCalendarAsked`): the start check asks once per
    // device.
    AsyncFunction("deviceCalendarAccessJson") {
      hostCoded {
        host.deviceCalendarAccessJson()
      }
    }

    // Aperio asked for the calendar permission on this device (decision 172).
    // The record lives outside Auto Backup, so a new phone is asked again.
    Function("noteDeviceCalendarAsked") {
      appContext.reactContext?.applicationContext?.let { AndroidDeviceCalendar.noteAsked(it) }
      Unit
    }

    // Force a full cold re-sync of one external account (clears its delta tokens
    // + cached window, then kicks a warm pass). The recovery action for a "stuck"
    // external cache; credentials are untouched.
    AsyncFunction("resetAccountSync") { accountId: String ->
      hostCoded {
        host.resetAccountSync(accountId)
      }
    }

    AsyncFunction("listAccountsMissingCredentialsJson") { lang: String? ->
      hostCoded {
        host.listAccountsMissingCredentialsJson(lang)
      }
    }.runOnQueue(readScope)

    AsyncFunction("setAccountSecret") { accountId: String, secret: String ->
      hostCoded {
        host.setAccountSecret(accountId, secret)
      }
    }
  }

  /** Lifted out of `definition()` — see the note there. Calendars and events. */
  private fun ModuleDefinitionBuilder.calendarFunctions() {
    // ─── Calendars + events (the on-device adapters, local + external) ───────
    // JSON passthrough in the cal_core/desktop wire shape. Routing (local vs
    // external account) happens Rust-side in the Host; a thrown StoreException
    // rejects the JS promise with the typed error (NotFound / Conflict / Auth /
    // …) the mobile api-client maps.

    AsyncFunction("listCalendarsJson") {
      hostCoded {
        host.listCalendarsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("createCalendarJson") { requestJson: String ->
      hostCoded {
        host.createCalendarJson(requestJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteCalendar") { id: String ->
      hostCoded {
        host.deleteCalendar(id)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("getEventsJson") { requestJson: String ->
      hostCoded {
        host.getEventsJson(requestJson)
      }
    }.runOnQueue(readScope)

    AsyncFunction("getEventByIdJson") { id: String, calendarId: String? ->
      hostCoded {
        host.getEventByIdJson(id, calendarId)
      }
    }.runOnQueue(readScope)

    AsyncFunction("queryFreeBusyJson") { requestJson: String ->
      hostCoded {
        host.queryFreeBusyJson(requestJson)
      }
    }.runOnQueue(slowScope)

    AsyncFunction("createEventJson") { requestJson: String ->
      hostCoded { host.createEventJson(requestJson) }
    }.runOnQueue(writeScope)

    AsyncFunction("updateEventJson") { eventJson: String, previousCalendarId: String? ->
      hostCoded { host.updateEventJson(eventJson, previousCalendarId) }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteEvent") { id: String, calendarId: String?, sendCancellations: Boolean? ->
      hostCoded { host.deleteEvent(id, calendarId, sendCancellations) }
    }.runOnQueue(writeScope)

    AsyncFunction("addEventExdateJson") { id: String, occurrence: String, calendarId: String?, sendCancellations: Boolean ->
      hostCoded { host.addEventExdateJson(id, occurrence, calendarId, sendCancellations) }
    }.runOnQueue(writeScope)
  }

  /** Lifted out of `definition()` — see the note there. Importing a file (DESIGN-file-import.md). */
  private fun ModuleDefinitionBuilder.fileImportFunctions() {
    // The formats the enabled plugins read (the document picker's filter), and
    // one file read through the plugin for its format. Nothing is written.

    AsyncFunction("fileImportFormatsJson") { lang: String ->
      hostCoded { host.fileImportFormatsJson(lang) }
    }

    AsyncFunction("readImportFileJson") { path: String ->
      importCoded { host.readImportFileJson(path) }
    }

    // A calendar file shared ("Send") or opened ("Open with") into Aperio while
    // it runs; one that started it is the activity's own intent.
    OnNewIntent { intent ->
      incomingIntent = intent
    }

    AsyncFunction("takeIncomingFileJson") {
      takeIncomingFile()
    }
  }

  /** The intent a share or an "open with" brought while the app ran. */
  @Volatile private var incomingIntent: Intent? = null

  /** The intents already read in this process, by identity (`Intent` has no
   *  equals of its own), so the activity's launch intent is taken once, not on
   *  every return to the foreground. */
  private val takenIntents: MutableSet<Intent> =
    java.util.Collections.synchronizedSet(
      java.util.Collections.newSetFromMap(java.util.WeakHashMap<Intent, Boolean>()),
    )

  /**
   * The calendar file a share or an "open with" brought (DESIGN-file-import.md,
   * I5), copied into the cache, as JSON `{path, name}` — or `null` when there
   * is none, or it was taken already. A file that cannot be read or copied is
   * an `internal` error carrying the file's name, so the person hears that it
   * could not be opened rather than nothing.
   *
   * Android hands a `content://` URI, a stream without a path (a share puts it
   * in `EXTRA_STREAM`, an "open with" in the intent's data), and the plugin
   * reads a path with `std::fs`. So the bytes are copied first, at most one
   * past the import's limit (`host_core::file_import::MAX_IMPORT_BYTES`): the
   * read then refuses a larger file as too large without the whole of it on
   * disk. A VIEW intent of another scheme (the OAuth callback) is not a file.
   *
   * The activity's own intent is the file that started the app. It stays the
   * activity's intent when Android recreates the activity after the process
   * was ended — from Recents (`FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY`), or when
   * the task is brought back — so a launch intent once taken is remembered by
   * its task and URI across processes; a new share that starts the app makes
   * a new task.
   */
  private fun takeIncomingFile(): String? {
    val fresh = incomingIntent
    incomingIntent = null
    val activity = appContext.currentActivity
    val intent = fresh ?: activity?.intent ?: return null
    val uri: Uri = when (intent.action) {
      Intent.ACTION_VIEW -> intent.data
      // Android puts a share's EXTRA_STREAM into the intent's ClipData as it
      // starts the activity (Intent.migrateExtraStreamToClipData), readable
      // without the getParcelableExtra this androidx.core does not wrap.
      Intent.ACTION_SEND -> intent.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
      else -> null
    } ?: return null
    if (uri.scheme != "content" && uri.scheme != "file") return null
    if (!takenIntents.add(intent)) return null
    val context = appContext.reactContext?.applicationContext ?: return null
    if (fresh == null) {
      if ((intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) != 0) return null
      val launch = "${activity?.taskId}|$uri"
      val taken = context.getSharedPreferences(INCOMING_PREFS, Context.MODE_PRIVATE)
      if (taken.getString(TAKEN_LAUNCH, null) == launch) return null
      taken.edit().putString(TAKEN_LAUNCH, launch).apply()
    }
    val name = incomingName(context, intent, uri)
    try {
      val dir = File(context.cacheDir, "incoming").apply { mkdirs() }
      val target = File(dir, name.replace('/', '_').replace('\\', '_'))
      val input = context.contentResolver.openInputStream(uri)
        ?: throw CodedException("internal", name, null)
      input.use {
        target.outputStream().use { output ->
          val buffer = ByteArray(64 * 1024)
          var left = INCOMING_CAP
          while (left > 0) {
            val read = input.read(buffer, 0, minOf(buffer.size.toLong(), left).toInt())
            if (read < 0) break
            output.write(buffer, 0, read)
            left -= read
          }
        }
      }
      return JSONObject().put("path", target.absolutePath).put("name", name).toString()
    } catch (e: CodedException) {
      throw e
    } catch (e: Exception) {
      // Not opened (no permission, gone, not a file): said with its name.
      throw CodedException("internal", name, e)
    }
  }

  /**
   * The name the file is known by: the one the sending app gives, else, for a
   * plain file, its own; with the extension of its MIME type where it has
   * none, since the import finds the plugin for a file by its extension. A
   * content URI's last segment is an opaque id, never a name.
   */
  private fun incomingName(context: Context, intent: Intent, uri: Uri): String {
    val given = displayName(context, uri)
      ?: (if (uri.scheme == "file") uri.lastPathSegment else null)?.takeIf { it.isNotBlank() }
      ?: "import"
    if (given.substringAfterLast('.', "").isNotEmpty()) return given
    val type = runCatching { context.contentResolver.getType(uri) }.getOrNull() ?: intent.type
    val extension = type?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
    return if (extension.isNullOrEmpty()) given else "$given.$extension"
  }

  /** The name the sending app gives the file, where it gives one. */
  private fun displayName(context: Context, uri: Uri): String? =
    runCatching {
      context.contentResolver
        .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
    }.getOrNull()?.takeIf { it.isNotBlank() }

  /** Lifted out of `definition()` — see the note there. User preferences, colour labels, day markers, search and what sits with them. */
  private fun ModuleDefinitionBuilder.preferenceFunctions() {
    // ─── User preferences (generic key/value; synced-key whitelist) ───────────
    // Opaque string values; a whitelisted key change appends a SettingsUpdated
    // sync event Rust-side so it propagates across devices.

    AsyncFunction("getUserPref") { key: String ->
      hostCoded {
        host.getUserPref(key)
      }
    }

    AsyncFunction("setUserPref") { key: String, value: String ->
      hostCoded {
        host.setUserPref(key, value)
      }
    }

    AsyncFunction("deleteUserPref") { key: String ->
      hostCoded {
        host.deleteUserPref(key)
      }
    }

    // ─── Colour labels (app-wide palette; local-only, always synced) ──────────

    AsyncFunction("listDayMarkersJson") { ->
      hostCoded {
        host.listDayMarkersJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("createDayMarkerJson") { name: String, symbol: String?, colorLabel: String? ->
      hostCoded {
        host.createDayMarkerJson(name, symbol, colorLabel)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("updateDayMarkerJson") { markerJson: String ->
      hostCoded {
        host.updateDayMarkerJson(markerJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteDayMarker") { id: String ->
      hostCoded {
        host.deleteDayMarker(id)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("dayLogJson") { day: String ->
      hostCoded {
        host.dayLogJson(day)
      }
    }.runOnQueue(readScope)

    AsyncFunction("dayLogsInRangeJson") { from: String, to: String ->
      hostCoded {
        host.dayLogsInRangeJson(from, to)
      }
    }.runOnQueue(readScope)

    AsyncFunction("setDayLogJson") { logJson: String ->
      hostCoded {
        host.setDayLogJson(logJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("listColorLabelsJson") { ->
      hostCoded {
        host.listColorLabelsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("createColorLabelJson") { name: String, hex: String ->
      hostCoded {
        host.createColorLabelJson(name, hex)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("getOrCreateAdHocColorLabelJson") { hex: String ->
      hostCoded {
        host.getOrCreateAdHocColorLabelJson(hex)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("updateColorLabelJson") { labelJson: String ->
      hostCoded {
        host.updateColorLabelJson(labelJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteColorLabel") { id: String ->
      hostCoded {
        host.deleteColorLabel(id)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("groupEventsJson") { membersJson: String ->
      groupCoded { host.groupEventsJson(membersJson) }
    }.runOnQueue(writeScope)

    AsyncFunction("ungroupEventJson") { calendarId: String, eventId: String, bookkeeping: Boolean ->
      hostCoded {
        host.ungroupEventJson(calendarId, eventId, bookkeeping)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("dissolveEventGroup") { groupId: String ->
      hostCoded {
        host.dissolveEventGroup(groupId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("eventGroupsForEventsJson") { eventsJson: String ->
      hostCoded {
        host.eventGroupsForEventsJson(eventsJson)
      }
    }.runOnQueue(readScope)

    AsyncFunction("declineGroupSuggestionJson") { firstJson: String, secondJson: String ->
      hostCoded {
        host.declineGroupSuggestionJson(firstJson, secondJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("groupSuggestionDeclinesJson") { ->
      hostCoded {
        host.groupSuggestionDeclinesJson()
      }
    }.runOnQueue(readScope)

    // Reminders Aperio keeps for one event and tells no provider about
    // (migration 0043).
    AsyncFunction("eventLocalRemindersJson") {
      hostCoded {
        host.eventLocalRemindersJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("setEventLocalRemindersJson") {
      calendarId: String, eventId: String, remindersJson: String, title: String, startsAt: String ->
      hostCoded {
        host.setEventLocalRemindersJson(calendarId, eventId, remindersJson, title, startsAt)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("healEventLocalReminders") {
      calendarId: String, oldEventId: String, newEventId: String ->
      hostCoded {
        host.healEventLocalReminders(calendarId, oldEventId, newEventId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("refreshEventLocalReminderSignature") {
      calendarId: String, eventId: String, title: String, startsAt: String ->
      hostCoded {
        host.refreshEventLocalReminderSignature(calendarId, eventId, title, startsAt)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("setContainerColorLabel") { containerId: String, kind: String, colorLabelId: String? ->
      hostCoded {
        host.setContainerColorLabel(containerId, kind, colorLabelId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("renameContainer") { containerId: String, kind: String, name: String ->
      hostCoded {
        host.renameContainer(containerId, kind, name)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("setSectionColor") { sectionId: String, listId: String, colorLabelId: String? ->
      hostCoded {
        host.setSectionColor(sectionId, listId, colorLabelId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("setEventColor") { eventId: String, calendarId: String, colorLabelId: String? ->
      hostCoded {
        host.setEventColor(eventId, calendarId, colorLabelId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("searchJson") { query: String, filtersJson: String ->
      hostCoded {
        host.searchJson(query, filtersJson)
      }
    }.runOnQueue(readScope)

    AsyncFunction("searchContactsJson") { query: String ->
      hostCoded {
        host.searchContactsJson(query)
      }
    }.runOnQueue(lookupScope)
  }

  /** Lifted out of `definition()` — see the note there. Collaboration: RSVP and task-list members and sharing. */
  private fun ModuleDefinitionBuilder.collaborationFunctions() {
    // ─── Collaboration: RSVP (§7.3) + task-list members/sharing (§9.7) ────────
    // Routed Rust-side to the owning external adapter; reads degrade to empty /
    // null for local + unroutable accounts (the UI hides the affordance), writes
    // throw. respondToEvent invalidates the event cache so the next read shows
    // the new status.

    AsyncFunction("calendarCurrentUserEmail") { calendarId: String ->
      hostCoded {
        host.calendarCurrentUserEmail(calendarId)
      }
    }.runOnQueue(lookupScope)

    AsyncFunction("respondToEvent") { calendarId: String, eventId: String, status: String, sendResponse: Boolean ->
      hostCoded { host.respondToEvent(calendarId, eventId, status, sendResponse) }
    }.runOnQueue(writeScope)

    // Coded, so a refused read keeps its token (`cal_core::ReadRefusal`) and
    // the editor can say which permission the account's token lacks.
    AsyncFunction("taskListMembersJson") { listId: String ->
      hostCoded { host.taskListMembersJson(listId) }
    }.runOnQueue(lookupScope)

    AsyncFunction("taskCurrentUserJson") { listId: String ->
      hostCoded {
        host.taskCurrentUserJson(listId)
      }
    }.runOnQueue(lookupScope)

    AsyncFunction("taskListSharesJson") { listId: String ->
      hostCoded {
        host.taskListSharesJson(listId)
      }
    }.runOnQueue(lookupScope)

    AsyncFunction("taskSearchUsersJson") { listId: String, query: String ->
      hostCoded {
        host.taskSearchUsersJson(listId, query)
      }
    }.runOnQueue(lookupScope)

    AsyncFunction("taskAddMember") { listId: String, memberRef: String, right: String? ->
      hostCoded {
        host.taskAddMember(listId, memberRef, right)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("taskRemoveMember") { listId: String, memberRef: String ->
      hostCoded {
        host.taskRemoveMember(listId, memberRef)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("taskSetMemberRight") { listId: String, memberRef: String, right: String ->
      hostCoded {
        host.taskSetMemberRight(listId, memberRef, right)
      }
    }.runOnQueue(writeScope)
  }

  /** Lifted out of `definition()` — see the note there. OAuth, discovery, sync-target OAuth and end-to-end encryption. */
  private fun ModuleDefinitionBuilder.oauthFunctions() {
    // ─── OAuth (host-driven; mobile opens authorize_url in a native session) ──
    // beginOauthJson runs the pure authorize phase (no network) → returns
    // {authorize_url, pkce_verifier, state}. complete (network exchange + account
    // creation) follows in a later phase.

    AsyncFunction("beginOauthJson") { pluginId: String, argsJson: String ->
      hostCoded {
        host.beginOauthJson(pluginId, argsJson)
      }
    }



    // ─── Discovery (EWS Autodiscover; host-driven, like the desktop) ──────────
    // discoverJson runs a plugin's endpoint discovery (EWS: {email, password} →
    // {ews_url, account_email}); the network call hits the provider, so a thrown
    // StoreException rejects the JS promise with the plugin's actionable message.

    AsyncFunction("discoverJson") { pluginId: String, argsJson: String ->
      hostCoded {
        host.discoverJson(pluginId, argsJson)
      }
    }.runOnQueue(slowScope)

    // ─── Sync-target OAuth (Dropbox / Google Drive) ───────────────────────────
    // completeSyncOauthJson exchanges the redirect's code for tokens (network)
    // and stores the refresh token in the adapter's keychain slot; the JS layer
    // then calls configureSyncAdapterJson({kind:"dropbox"|"googledrive", …}).

    AsyncFunction("completeSyncOauthJson") { pluginId: String, requestJson: String ->
      hostCoded {
        host.completeSyncOauthJson(pluginId, requestJson)
      }
    }.runOnQueue(slowScope)

    // ─── E2E sync encryption (§19.7) ──────────────────────────────────────────
    // enableSyncEncryptionJson turns on E2E for the configured target (mint key,
    // write the encrypted dataset, encrypt every subsequent round).

    AsyncFunction("enableSyncEncryptionJson") { passphrase: String ->
      hostCoded { host.enableSyncEncryptionJson(passphrase) }
    }.runOnQueue(slowScope)

    // disableSyncEncryptionJson turns E2E OFF: rewrites every log + snapshot as
    // plaintext, flips the meta, drops the device key (other devices re-onboard).
    AsyncFunction("disableSyncEncryptionJson") { passphrase: String ->
      hostCoded { host.disableSyncEncryptionJson(passphrase) }
    }.runOnQueue(slowScope)

    // changeSyncPassphraseJson rotates the E2E passphrase (re-wraps the same
    // data key; existing devices keep working, future joins need the new one).
    AsyncFunction("changeSyncPassphraseJson") { oldPassphrase: String, newPassphrase: String ->
      hostCoded { host.changeSyncPassphraseJson(oldPassphrase, newPassphrase) }
    }.runOnQueue(slowScope)

    // adoptRemoteEncryptionJson: a peer turned E2E on while this device synced
    // plaintext; derive the key from the passphrase + swap to an encrypting
    // adapter so the next round (which had failed with encryption_required) works.
    AsyncFunction("adoptRemoteEncryptionJson") { passphrase: String ->
      hostCoded { host.adoptRemoteEncryptionJson(passphrase) }
    }.runOnQueue(slowScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.syncFunctions() {
    // ─── Sync (full desktop peer: same engine, statically-embedded adapters) ──
    // configure sets the active sync target; sync_now runs a round + returns the
    // report; a thrown StoreException rejects the JS promise.

    AsyncFunction("configureSyncAdapterJson") { configJson: String ->
      hostCoded { host.configureSyncAdapterJson(configJson) }
    }.runOnQueue(slowScope)

    // The sync SCREEN's verb: point this device at an account it already has.
    // Probes the target before it commits, so it belongs on the slow queue with
    // the other network-touching calls.
    AsyncFunction("selectSyncAccount") { accountId: String ->
      hostCoded { host.selectSyncAccount(accountId) }
    }.runOnQueue(slowScope)

    AsyncFunction("syncStatusJson") {
      hostCoded { host.syncStatusJson() }
    }.runOnQueue(readScope)

    AsyncFunction("syncNowJson") { trigger: String ->
      hostCoded { host.syncNowJson(trigger) }
    }.runOnQueue(slowScope)

    AsyncFunction("disconnectSync") {
      hostCoded {
        host.disconnectSync()
      }
    }

    AsyncFunction("getSyncAdapterSummaryJson") {
      hostCoded {
        host.getSyncAdapterSummaryJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("pushNow") { trigger: String ->
      hostCoded {
        host.pushNow(trigger).toInt()
      }
    }.runOnQueue(slowScope)

    AsyncFunction("listSyncLogJson") { limit: Int ->
      hostCoded {
        host.listSyncLogJson(limit.toUInt())
      }
    }.runOnQueue(readScope)

    AsyncFunction("clearSyncLog") {
      hostCoded {
        host.clearSyncLog()
      }
    }

    AsyncFunction("compactNowJson") {
      hostCoded { host.compactNowJson() }
    }.runOnQueue(slowScope)

    AsyncFunction("refreshExternalCache") {
      hostCoded {
        host.refreshExternalCache()
      }
    }

    AsyncFunction("refreshAccountCache") { accountId: String ->
      hostCoded {
        host.refreshAccountCache(accountId)
      }
    }

    AsyncFunction("refreshExternalCacheInBackground") {
      hostCoded {
        host.refreshExternalCacheInBackground()
      }
    }

    AsyncFunction("getCacheRefreshStatusJson") {
      hostCoded {
        host.getCacheRefreshStatusJson()
      }
    }.runOnQueue(readScope)

    // Per-account refresh-error surface (silent-staleness warning).
    AsyncFunction("refreshErrorsJson") {
      hostCoded {
        host.refreshErrorsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("warmCacheOnForeground") {
      hostCoded {
        host.warmCacheOnForeground()
      }
    }

    // Contact sync (§10.5). The pass is driven from JS (manual button /
    // foreground); the interval + include-read-only prefs are device-local.
    AsyncFunction("syncContactsNow") { includeReadOnly: Boolean? ->
      hostCoded { host.syncContactsNow(includeReadOnly) }
    }.runOnQueue(slowScope)

    AsyncFunction("getContactsSyncStatusJson") {
      hostCoded {
        host.getContactsSyncStatusJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("setContactsSyncInterval") { minutes: Int ->
      hostCoded {
        host.setContactsSyncInterval(minutes.toUInt()).toInt()
      }
    }

    AsyncFunction("setContactsIncludeReadOnlyOnSync") { enabled: Boolean ->
      hostCoded {
        host.setContactsIncludeReadOnlyOnSync(enabled)
      }
    }

    AsyncFunction("clearContactsCache") {
      hostCoded {
        host.clearContactsCache().toInt()
      }
    }

    // Diagnostics / logs (§ Diagnostics).
    AsyncFunction("getLogLevel") {
      hostCoded {
        host.getLogLevel()
      }
    }

    AsyncFunction("setLogLevel") { level: String ->
      hostCoded {
        host.setLogLevel(level)
      }
    }

    AsyncFunction("getRecentLogs") { lines: Int? ->
      hostCoded {
        host.getRecentLogs(lines?.toUInt())
      }
    }

    AsyncFunction("collectLogs") { redact: Boolean? ->
      hostCoded {
        host.collectLogs(redact)
      }
    }

    AsyncFunction("clearLogs") {
      hostCoded {
        host.clearLogs()
      }
    }

    AsyncFunction("logsDirPath") {
      hostCoded {
        host.logsDirPath()
      }
    }

    AsyncFunction("syncConflictCount") {
      hostCoded { host.syncConflictCount().toInt() }
    }.runOnQueue(readScope)

    AsyncFunction("listSyncConflictsJson") {
      hostCoded {
        host.listSyncConflictsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("resolveSyncConflict") { id: Int, choice: String ->
      hostCoded {
        host.resolveSyncConflict(id.toLong(), choice)
      }
    }.runOnQueue(writeScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.soundFunctions() {
    // ─── Custom reminder sounds (§14.4 / §19.2.2) ─────────────────────────────
    // Content-addressed audio store behind SoundSource::Custom; the sync round
    // push/fetches it already. Bytes don't cross the bridge — the JS plays +
    // builds the Android notification channel from the on-disk path.

    AsyncFunction("importSoundJson") { path: String ->
      hostCoded {
        host.importSoundJson(path)
      }
    }

    AsyncFunction("listCustomSoundsJson") { ->
      hostCoded {
        host.listCustomSoundsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("customSoundPath") { sha256: String ->
      hostCoded {
        host.customSoundPath(sha256)
      }
    }

    AsyncFunction("deleteCustomSound") { sha256: String ->
      hostCoded {
        host.deleteCustomSound(sha256)
      }
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
      hostCoded {
        host.contactListsJson()
      }
    }.runOnQueue(readScope)

    AsyncFunction("contactsJson") { listId: String ->
      hostCoded {
        host.contactsJson(listId)
      }
    }.runOnQueue(readScope)

    AsyncFunction("createContactJson") { listId: String, contactJson: String ->
      hostCoded {
        host.createContactJson(listId, contactJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("updateContactJson") { contactJson: String ->
      hostCoded {
        host.updateContactJson(contactJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteContact") { id: String, listId: String? ->
      hostCoded {
        host.deleteContact(id, listId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("getContactPhotoJson") { id: String, listId: String? ->
      hostCoded {
        host.getContactPhotoJson(id, listId)
      }
    }.runOnQueue(lookupScope)

    AsyncFunction("setContactPhotoJson") { id: String, listId: String?, photoJson: String ->
      hostCoded {
        host.setContactPhotoJson(id, listId, photoJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteContactPhoto") { id: String, listId: String? ->
      hostCoded {
        host.deleteContactPhoto(id, listId)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("createContactListJson") { name: String ->
      hostCoded {
        host.createContactListJson(name)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("deleteContactList") { id: String ->
      hostCoded {
        host.deleteContactList(id)
      }
    }.runOnQueue(writeScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.meetingFunctions() {
    // ─── Meetings (network: they talk to the provider) ───────────────────────

    AsyncFunction("attachMeetingJson") { requestJson: String ->
      hostCoded {
        host.attachMeetingJson(requestJson)
      }
    }.runOnQueue(slowScope)

    AsyncFunction("detachMeetingJson") { requestJson: String ->
      hostCoded {
        host.detachMeetingJson(requestJson)
      }
    }.runOnQueue(slowScope)

    // Network: it asks the provider about the meeting.
    AsyncFunction("inspectEventMeetingJson") { requestJson: String ->
      hostCoded {
        host.inspectEventMeetingJson(requestJson)
      }
    }.runOnQueue(slowScope)

    AsyncFunction("adoptMeetingJson") { requestJson: String ->
      hostCoded {
        host.adoptMeetingJson(requestJson)
      }
    }.runOnQueue(writeScope)

    AsyncFunction("eventMeetingJson") { eventId: String, calendarId: String? ->
      hostCoded {
        host.eventMeetingJson(eventId, calendarId)
      }
    }.runOnQueue(readScope)

    AsyncFunction("listAdapterKindsJson") { lang: String? ->
      hostCoded {
        host.listAdapterKindsJson(lang)
      }
    }

    AsyncFunction("accountFormSpecJson") { adapterKind: String, lang: String? ->
      hostCoded {
        host.accountFormSpecJson(adapterKind, lang)
      }
    }

    // Pure (no network): builds the consent URL + PKCE verifier + state.
    AsyncFunction("beginAccountOauthJson") { adapterKind: String, valuesJson: String ->
      hostCoded {
        host.beginAccountOauthJson(adapterKind, valuesJson)
      }
    }

    // Network: the token exchange, then the row + keychain + registration.
    AsyncFunction("connectAccountJson") { requestJson: String ->
      hostCoded {
        host.connectAccountJson(requestJson)
      }
    }.runOnQueue(slowScope)

    // Re-sign-in for an EXISTING account. Both halves take only the account id:
    // everything else — which client, which redirect, whether there is a client
    // secret at all — the host reads off the account and its adapter's schema.
    // `hostCoded`, so `client_secret_required` reaches JS as error.code — the
    // second-device repair asks for the secret and retries on exactly that.
    AsyncFunction("beginAccountReconnectJson") { accountId: String ->
      hostCoded { host.beginAccountReconnectJson(accountId) }
    }

    AsyncFunction("completeAccountReconnectJson") { accountId: String, requestJson: String ->
      hostCoded { host.completeAccountReconnectJson(accountId, requestJson) }
    }.runOnQueue(slowScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.onboardingFunctions() {
    // ─── Onboarding: preview + join an existing dataset (§19.11) ──────────────
    // previewSyncTargetJson reads the target's meta.json WITHOUT committing →
    // {kind: empty | existing, …}. acceptRemoteDatasetJson joins an existing
    // dataset (deriving the E2E key from the passphrase when it's encrypted).

    AsyncFunction("previewSyncTargetJson") { configJson: String ->
      hostCoded { host.previewSyncTargetJson(configJson) }
    }.runOnQueue(slowScope)

    AsyncFunction("previewSyncTargetValuesJson") { requestJson: String ->
      hostCoded { host.previewSyncTargetValuesJson(requestJson) }
    }.runOnQueue(slowScope)

    AsyncFunction("acceptRemoteDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      hostCoded { host.acceptRemoteDatasetJson(configJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    // adoptLocalDatasetJson initialises a FRESH dataset (the unified Connect
    // button's empty-target path), optionally enabling E2E at creation.
    AsyncFunction("adoptLocalDatasetJson") { configJson: String, deviceName: String?, passphrase: String? ->
      hostCoded { host.adoptLocalDatasetJson(configJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    // The same two answers, asked with the shared schema form's own values, and
    // committing to an ACCOUNT ROW rather than to device-local preferences.
    AsyncFunction("acceptRemoteDatasetValuesJson") { requestJson: String, deviceName: String?, passphrase: String? ->
      hostCoded { host.acceptRemoteDatasetValuesJson(requestJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    AsyncFunction("adoptLocalDatasetValuesJson") { requestJson: String, deviceName: String?, passphrase: String? ->
      hostCoded { host.adoptLocalDatasetValuesJson(requestJson, deviceName, passphrase) }
    }.runOnQueue(slowScope)

    AsyncFunction("resumeStaleDeviceJson") {
      hostCoded { host.resumeStaleDeviceJson() }
    }.runOnQueue(slowScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.sftpFunctions() {
    // ─── SFTP host-key trust (§19.5 TOFU) ─────────────────────────────────────
    // previewSftpHostKeyJson probes the server's fingerprint (network) + compares
    // it to the device pin store → {host_port, fingerprint, status}; trust/forget/
    // pinned manage the pin (no network). The JS layer shows the trust dialog.

    AsyncFunction("previewSftpHostKeyJson") { argsJson: String ->
      hostCoded { host.previewSftpHostKeyJson(argsJson) }
    }.runOnQueue(slowScope)

    // The same probe for an account the user is about to sync through — the
    // repair for a selectSyncAccount that refused an unconfirmed host key.
    // Returns the JSON `null` (no network) when the adapter pins no host key.
    AsyncFunction("previewSyncAccountHostKeyJson") { accountId: String ->
      hostCoded { host.previewSyncAccountHostKeyJson(accountId) }
    }.runOnQueue(slowScope)

    // Read-only, and no network: what this device already confirmed.
    AsyncFunction("syncAccountHostKeyPinJson") { accountId: String ->
      hostCoded { host.syncAccountHostKeyPinJson(accountId) }
    }

    AsyncFunction("trustSftpHostKey") { hostPort: String, fingerprint: String ->
      hostCoded { host.trustSftpHostKey(hostPort, fingerprint) }
    }

    AsyncFunction("forgetSftpHostKey") { hostPort: String ->
      hostCoded {
        host.forgetSftpHostKey(hostPort)
      }
    }

    AsyncFunction("pinnedSftpHostKey") { hostPort: String ->
      hostCoded {
        host.pinnedSftpHostKey(hostPort)
      }
    }

    // §19 device registry: this device's own name, and the list of everyone the
    // dataset still counts as a participant.
    AsyncFunction("syncDeviceNameJson") {
      hostCoded { host.syncDeviceNameJson() }
    }

    AsyncFunction("setSyncDeviceName") { name: String ->
      hostCoded { host.setSyncDeviceName(name) }
    }

    AsyncFunction("listSyncDevicesJson") {
      hostCoded { host.listSyncDevicesJson() }
    }.runOnQueue(lookupScope)

    AsyncFunction("forgetSyncDevice") { deviceId: String ->
      hostCoded { host.forgetSyncDevice(deviceId) }
    }.runOnQueue(slowScope)
  }

  /** Lifted out of `definition()` — see the note there. */
  private fun ModuleDefinitionBuilder.widgetFunctions() {
    // ── Widgets ──
    // The same snapshot document iOS uses, in the app's own internal storage —
    // an Android widget runs in this very process under the same uid, so there
    // is no App Group to cross and no container to resolve.
    // One line into the app's own rolling log — the twin of the iOS registration.
    AsyncFunction("logLine") { level: String, message: String ->
      hostCoded {
        host.logLine(level, message)
      }
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
