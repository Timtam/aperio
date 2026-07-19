import ExpoModulesCore
import Foundation

// Calls the Rust `cal_ffi::parse_attendee` through the UniFFI-generated Swift
// bindings (cal_ffi.swift, compiled into this module) backed by
// CalFfi.xcframework. Engine reuse: the same cal-core parser the desktop and
// the Android build use. Mirrors the Android CalFfiModule.

/// Adapts the UniFFI `CacheObserverBridge` callback to Expo events. A finished
/// background refresh / warm pass calls back here (on a background thread);
/// `sendEvent` forwards it to JS. Mirrors the Android `JsCacheObserver`.
private final class JsCacheObserver: CacheObserverBridge {
  weak var module: CalFfiModule?
  init(module: CalFfiModule) { self.module = module }
  func cacheUpdated(payloadJson: String) {
    module?.sendEvent("onCacheUpdated", ["payload": payloadJson])
  }
  func refreshStatus(statusJson: String) {
    module?.sendEvent("onCacheRefreshStatus", ["status": statusJson])
  }
}

/// Adapts the UniFFI `ContactSyncObserverBridge` callback to an Expo event. A
/// finished contact-sync pass calls back here (on a background thread);
/// `sendEvent` forwards it to JS. Mirrors the Android `JsContactSyncObserver`.
private final class JsContactSyncObserver: ContactSyncObserverBridge {
  weak var module: CalFfiModule?
  init(module: CalFfiModule) { self.module = module }
  func contactsSynced(payloadJson: String) {
    module?.sendEvent("onContactsSynced", ["payload": payloadJson])
  }
}

public class CalFfiModule: Module {
  // Expo runs EVERY AsyncFunction body on ONE shared serial queue
  // ("expo.modules.AsyncFunctionQueue"), so a seconds-long sync round makes
  // every read call queue behind it — on app open the views then sit on
  // "Loading…" until the launch sync finishes, even with a warm SWR cache.
  // The long NETWORK-bound operations therefore run on their own serial
  // queue: sync rounds stay mutually exclusive among themselves, while the
  // short reads/writes on the default queue flow past them. (The Rust Host is
  // Send+Sync — uniffi's contract — and its SQLite access is mutex-guarded,
  // so both queues may safely call into it concurrently.)
  private let slowQueue = DispatchQueue(label: "expo.modules.calffi.slow", qos: .userInitiated)

  // The full on-device engine: accounts + the statically-embedded adapter
  // registry, opened at the app-sandbox database path. Credentials route
  // through IosKeychain (Security-framework Keychain). Mirrors the Android
  // module's `host`.
  //
  // Opening is EXPENSIVE (DB open + migrations + read pool + 17 static
  // plugin registrations + tokio runtime + orchestrator build + cache
  // reconcile) and used to hide inside a `lazy var` — i.e. inside whatever
  // innocuous FIRST bridge call the JS side happened to make, on the
  // default serial AsyncFunction queue, with every later call waiting
  // behind it. `OnCreate` below kicks the open on the slow queue while
  // the JS bundle is still loading, so the first read pays (at most) the
  // REMAINDER of the open, not all of it. A `lazy var` is not
  // thread-safe, hence the explicit lock-guarded once-init.
  private let hostLock = NSLock()
  private var hostStorage: Host?
  private var host: Host {
    hostLock.lock()
    defer { hostLock.unlock() }
    if let opened = hostStorage {
      return opened
    }
    let opened = openHost()
    hostStorage = opened
    return opened
  }

  private func openHost() -> Host {
    let dir = try! FileManager.default.url(
      for: .applicationSupportDirectory,
      in: .userDomainMask,
      appropriateFor: nil,
      create: true
    )
    let dbPath = dir.appendingPathComponent("aperio.sqlite").path
    let opened = try! Host.open(dbPath: dbPath, keychain: IosKeychain())
    // Forward external-cache refresh callbacks to JS as Expo events (live-update
    // the open view + a polite announcement). Mirrors the Android module.
    opened.setCacheObserver(observer: JsCacheObserver(module: self))
    // Forward contact-sync pass-finished callbacks to JS. Mirrors the Android module.
    opened.setContactSyncObserver(observer: JsContactSyncObserver(module: self))
    // Install the device calendar/reminders bridge (EventKit). The Host registers
    // any persisted device-calendar account against it now; Android sets none, so
    // the device kind is iOS-only. Mirrors the keychain/observer injection above.
    opened.setDeviceEventStore(bridge: IosDeviceEventStore())
    return opened
  }

  public func definition() -> ModuleDefinition {
    Name("CalFfi")

    // External-cache push events (the mobile analogue of the desktop Tauri
    // cache-updated / cache-refresh-status events). onCacheUpdated carries
    // { payload: "<CacheUpdatedPayload JSON>" }; onCacheRefreshStatus carries
    // { status: "<CacheRefreshStatus JSON>" }.
    Events("onCacheUpdated", "onCacheRefreshStatus", "onContactsSynced")

    // Eager engine open (see `host`): start the expensive Host construction
    // off the critical path, while the JS bundle is still loading. The slow
    // queue keeps it clear of the default AsyncFunction queue; the lock in
    // the accessor makes a racing first call simply wait for the remainder.
    OnCreate {
      self.slowQueue.async { _ = self.host }
    }

    Function("parseAttendee") { (entry: String) -> [String: Any?] in
      let parsed = parseAttendee(entry: entry)
      return ["name": parsed.name, "email": parsed.email]
    }

    // ─── Tasks / lists / sections (JSON bridge, sync-logged) ───
    // The full task / list / section domain crosses as a JSON string in the
    // cal_core serde shape — identical to the desktop's Tauri payloads — so this
    // layer is a trivial passthrough; each Host mutation appends the matching
    // SyncEvent. Mirrors the Android module.

    AsyncFunction("taskListsJson") { () -> String in
      try self.host.taskListsJson()
    }

    AsyncFunction("createTaskListJson") { (name: String) -> String in
      try self.host.createTaskListJson(name: name)
    }

    AsyncFunction("reparentTaskListJson") { (id: String, parentId: String?) -> String in
      try self.host.reparentTaskListJson(id: id, parentId: parentId)
    }

    AsyncFunction("deleteTaskList") { (id: String) in
      try self.host.deleteTaskList(id: id)
    }

    AsyncFunction("tasksJson") { (listId: String) -> String in
      try self.host.tasksJson(listId: listId)
    }

    AsyncFunction("taskJson") { (id: String) -> String in
      try self.host.taskJson(id: id)
    }

    AsyncFunction("createTaskJson") { (listId: String, newTaskJson: String) -> String in
      try self.host.createTaskJson(listId: listId, newTaskJson: newTaskJson)
    }

    AsyncFunction("updateTaskJson") { (taskJson: String, previousListId: String?) -> String in
      try self.host.updateTaskJson(taskJson: taskJson, previousListId: previousListId)
    }

    AsyncFunction("deleteTask") { (taskId: String, listId: String?) in
      try self.host.deleteTask(id: taskId, listId: listId)
    }

    AsyncFunction("sectionsJson") { (listId: String) -> String in
      try self.host.sectionsJson(listId: listId)
    }

    AsyncFunction("createSectionJson") { (listId: String, name: String, position: Int, colorLabel: String?) -> String in
      try self.host.createSectionJson(
        listId: listId, name: name, position: UInt32(position), colorLabel: colorLabel)
    }

    AsyncFunction("updateSectionJson") { (sectionJson: String) -> String in
      try self.host.updateSectionJson(sectionJson: sectionJson)
    }

    AsyncFunction("deleteSection") { (id: String, listId: String?) in
      try self.host.deleteSection(id: id, listId: listId)
    }

    // ─── Accounts (the full engine: external adapters + secrets) ───
    // JSON passthrough in the cal_core/desktop wire shape; a thrown StoreError
    // rejects the JS promise. Mirrors the Android module.

    AsyncFunction("accountsJson") { () -> String in
      try self.host.accountsJson()
    }

    AsyncFunction("createAccountJson") { (requestJson: String) -> String in
      try self.host.createAccountJson(requestJson: requestJson)
    }

    AsyncFunction("testAccountJson") { (requestJson: String) in
      try self.host.testAccountJson(requestJson: requestJson)
    }.runOnQueue(slowQueue)

    AsyncFunction("deleteAccount") { (accountId: String) in
      try self.host.deleteAccount(accountId: accountId)
    }

    AsyncFunction("renameAccountJson") { (id: String, newName: String) -> String in
      try self.host.renameAccountJson(id: id, newName: newName)
    }

    // Run the OS calendar/reminders permission prompt for the device-calendar
    // adapter's add-account "grant access" step. `true` ⇒ proceed to
    // createAccountJson for the `device_calendar` kind. iOS-backed (EventKit);
    // on Android the Host has no bridge and this rejects "not available".
    AsyncFunction("requestDeviceCalendarAccess") { (events: Bool, reminders: Bool) -> Bool in
      try self.host.requestDeviceCalendarAccess(events: events, reminders: reminders)
    }

    // Force a full cold re-sync of one external account (clears its delta tokens
    // + cached window, then kicks a warm pass). The recovery action for a "stuck"
    // external cache; credentials are untouched.
    AsyncFunction("resetAccountSync") { (accountId: String) in
      try self.host.resetAccountSync(accountId: accountId)
    }

    AsyncFunction("listAccountsMissingCredentialsJson") { () -> String in
      try self.host.listAccountsMissingCredentialsJson()
    }

    AsyncFunction("setAccountSecret") { (accountId: String, secret: String) in
      try self.host.setAccountSecret(accountId: accountId, secret: secret)
    }

    // ─── Calendars + events (local + external adapters) ───
    // JSON passthrough; routing happens Rust-side. Mirrors the Android module.

    AsyncFunction("listCalendarsJson") { () -> String in
      try self.host.listCalendarsJson()
    }

    AsyncFunction("createCalendarJson") { (requestJson: String) -> String in
      try self.host.createCalendarJson(requestJson: requestJson)
    }

    AsyncFunction("deleteCalendar") { (id: String) in
      try self.host.deleteCalendar(id: id)
    }

    AsyncFunction("getEventsJson") { (requestJson: String) -> String in
      try self.host.getEventsJson(requestJson: requestJson)
    }

    AsyncFunction("getEventByIdJson") { (id: String, calendarId: String?) -> String in
      try self.host.getEventByIdJson(id: id, calendarId: calendarId)
    }

    AsyncFunction("queryFreeBusyJson") { (requestJson: String) -> String in
      try self.host.queryFreeBusyJson(requestJson: requestJson)
    }.runOnQueue(slowQueue)

    AsyncFunction("createEventJson") { (requestJson: String) -> String in
      try self.host.createEventJson(requestJson: requestJson)
    }

    AsyncFunction("updateEventJson") { (eventJson: String, previousCalendarId: String?) -> String in
      try self.host.updateEventJson(eventJson: eventJson, previousCalendarId: previousCalendarId)
    }

    AsyncFunction("deleteEvent") { (id: String, calendarId: String?, sendCancellations: Bool?) in
      try self.host.deleteEvent(id: id, calendarId: calendarId, sendCancellations: sendCancellations)
    }

    AsyncFunction("addEventExdateJson") { (id: String, occurrence: String, calendarId: String?, sendCancellations: Bool) in
      try self.host.addEventExdateJson(id: id, occurrence: occurrence, calendarId: calendarId, sendCancellations: sendCancellations)
    }

    // ─── Sync ───

    AsyncFunction("configureSyncAdapterJson") { (configJson: String) in
      try self.host.configureSyncAdapterJson(configJson: configJson)
    }.runOnQueue(slowQueue)

    AsyncFunction("syncStatusJson") { () -> String in
      try self.host.syncStatusJson()
    }

    AsyncFunction("syncNowJson") { (trigger: String) -> String in
      try self.host.syncNowJson(trigger: trigger)
    }.runOnQueue(slowQueue)

    AsyncFunction("disconnectSync") { () in
      try self.host.disconnectSync()
    }

    AsyncFunction("getSyncAdapterSummaryJson") { () -> String in
      try self.host.getSyncAdapterSummaryJson()
    }

    AsyncFunction("pushNow") { (trigger: String) -> Int in
      Int(try self.host.pushNow(trigger: trigger))
    }.runOnQueue(slowQueue)

    AsyncFunction("listSyncLogJson") { (limit: Int) -> String in
      try self.host.listSyncLogJson(limit: UInt32(limit))
    }

    AsyncFunction("clearSyncLog") {
      try self.host.clearSyncLog()
    }

    AsyncFunction("compactNowJson") { () -> String in
      try self.host.compactNowJson()
    }.runOnQueue(slowQueue)

    AsyncFunction("refreshExternalCache") {
      self.host.refreshExternalCache()
    }

    AsyncFunction("getCacheRefreshStatusJson") { () -> String in
      try self.host.getCacheRefreshStatusJson()
    }

    // Per-account refresh-error surface (silent-staleness warning).
    AsyncFunction("refreshErrorsJson") { () -> String in
      try self.host.refreshErrorsJson()
    }

    AsyncFunction("warmCacheOnForeground") {
      self.host.warmCacheOnForeground()
    }

    // Contact sync (§10.5). The pass is driven from JS (manual button /
    // foreground); the interval + include-read-only prefs are device-local.
    AsyncFunction("syncContactsNow") { (includeReadOnly: Bool?) -> Bool in
      try self.host.syncContactsNow(includeReadOnly: includeReadOnly)
    }.runOnQueue(slowQueue)

    AsyncFunction("getContactsSyncStatusJson") { () -> String in
      try self.host.getContactsSyncStatusJson()
    }

    AsyncFunction("setContactsSyncInterval") { (minutes: Int) -> Int in
      Int(try self.host.setContactsSyncInterval(minutes: UInt32(minutes)))
    }

    AsyncFunction("setContactsIncludeReadOnlyOnSync") { (enabled: Bool) in
      try self.host.setContactsIncludeReadOnlyOnSync(enabled: enabled)
    }

    AsyncFunction("clearContactsCache") { () -> Int in
      Int(try self.host.clearContactsCache())
    }

    // Diagnostics / logs (§ Diagnostics).
    AsyncFunction("getLogLevel") { () -> String in
      try self.host.getLogLevel()
    }

    AsyncFunction("setLogLevel") { (level: String) in
      try self.host.setLogLevel(level: level)
    }

    AsyncFunction("getRecentLogs") { (lines: Int?) -> String in
      try self.host.getRecentLogs(lines: lines.map { UInt32($0) })
    }

    AsyncFunction("collectLogs") { (redact: Bool?) -> String in
      try self.host.collectLogs(redact: redact)
    }

    AsyncFunction("clearLogs") {
      try self.host.clearLogs()
    }

    AsyncFunction("logsDirPath") { () -> String in
      try self.host.logsDirPath()
    }

    AsyncFunction("syncConflictCount") { () -> Int in
      Int(try self.host.syncConflictCount())
    }

    AsyncFunction("listSyncConflictsJson") { () -> String in
      try self.host.listSyncConflictsJson()
    }

    AsyncFunction("resolveSyncConflict") { (id: Int, choice: String) in
      try self.host.resolveSyncConflict(id: Int64(id), choice: choice)
    }

    // ─── Reminders ───
    // Upcoming reminder triggers (local + external) within a horizon, for the JS
    // layer to schedule as expo-notifications.

    AsyncFunction("upcomingRemindersJson") { (horizonMinutes: Int) -> String in
      try self.host.upcomingRemindersJson(horizonMinutes: UInt32(horizonMinutes))
    }

    // ─── Custom reminder sounds (§14.4 / §19.2.2) ─────────────────────────────
    // iOS can't use a runtime file as a notification sound, so here these drive
    // import + in-app preview + sync only; the notification falls back to default.

    AsyncFunction("importSoundJson") { (path: String) -> String in
      try self.host.importSoundJson(path: path)
    }

    AsyncFunction("listCustomSoundsJson") { () -> String in
      try self.host.listCustomSoundsJson()
    }

    AsyncFunction("customSoundPath") { (sha256: String) -> String? in
      try self.host.customSoundPath(sha256: sha256)
    }

    AsyncFunction("deleteCustomSound") { (sha256: String) in
      try self.host.deleteCustomSound(sha256: sha256)
    }

    // No-op on iOS: UNNotificationSound only plays sounds bundled into the app
    // at build time, so a runtime-imported custom sound can't drive a
    // notification here — the scheduler keeps the default sound. (Android creates
    // a per-sound channel; this stub keeps the JS surface uniform.)
    AsyncFunction("ensureCustomSoundChannel") { (_: String, _: String, _: String) in
    }

    // ─── User preferences (generic key/value; synced-key whitelist) ───
    AsyncFunction("getUserPref") { (key: String) -> String? in
      try self.host.getUserPref(key: key)
    }

    AsyncFunction("setUserPref") { (key: String, value: String) in
      try self.host.setUserPref(key: key, value: value)
    }

    AsyncFunction("deleteUserPref") { (key: String) in
      try self.host.deleteUserPref(key: key)
    }

    // ─── Colour labels (app-wide palette; local-only, always synced) ───

    AsyncFunction("listColorLabelsJson") { () -> String in
      try self.host.listColorLabelsJson()
    }

    AsyncFunction("createColorLabelJson") { (name: String, hex: String) -> String in
      try self.host.createColorLabelJson(name: name, hex: hex)
    }

    AsyncFunction("getOrCreateAdHocColorLabelJson") { (hex: String) -> String in
      try self.host.getOrCreateAdHocColorLabelJson(hex: hex)
    }

    AsyncFunction("updateColorLabelJson") { (labelJson: String) -> String in
      try self.host.updateColorLabelJson(labelJson: labelJson)
    }

    AsyncFunction("deleteColorLabel") { (id: String) in
      try self.host.deleteColorLabel(id: id)
    }

    AsyncFunction("setContainerColorLabel") { (containerId: String, kind: String, colorLabelId: String?) in
      try self.host.setContainerColorLabel(containerId: containerId, kind: kind, colorLabelId: colorLabelId)
    }

    AsyncFunction("renameContainer") { (containerId: String, kind: String, name: String) in
      try self.host.renameContainer(containerId: containerId, kind: kind, name: name)
    }

    AsyncFunction("setSectionColor") { (sectionId: String, listId: String, colorLabelId: String?) in
      try self.host.setSectionColor(sectionId: sectionId, listId: listId, colorLabelId: colorLabelId)
    }

    AsyncFunction("setEventColor") { (eventId: String, calendarId: String, colorLabelId: String?) in
      try self.host.setEventColor(eventId: eventId, calendarId: calendarId, colorLabelId: colorLabelId)
    }

    AsyncFunction("searchJson") { (query: String, filtersJson: String) -> String in
      try self.host.searchJson(query: query, filtersJson: filtersJson)
    }

    AsyncFunction("searchContactsJson") { (query: String) -> String in
      try self.host.searchContactsJson(query: query)
    }

    // ─── Contacts ───
    // JSON passthrough, routed Rust-side. Mirrors the Android module.

    AsyncFunction("contactListsJson") { () -> String in
      try self.host.contactListsJson()
    }

    AsyncFunction("contactsJson") { (listId: String) -> String in
      try self.host.contactsJson(listId: listId)
    }

    AsyncFunction("createContactJson") { (listId: String, contactJson: String) -> String in
      try self.host.createContactJson(listId: listId, contactJson: contactJson)
    }

    AsyncFunction("updateContactJson") { (contactJson: String) -> String in
      try self.host.updateContactJson(contactJson: contactJson)
    }

    AsyncFunction("deleteContact") { (id: String, listId: String?) in
      try self.host.deleteContact(id: id, listId: listId)
    }

    AsyncFunction("getContactPhotoJson") { (id: String, listId: String?) -> String in
      try self.host.getContactPhotoJson(id: id, listId: listId)
    }

    AsyncFunction("setContactPhotoJson") { (id: String, listId: String?, photoJson: String) in
      try self.host.setContactPhotoJson(id: id, listId: listId, photoJson: photoJson)
    }

    AsyncFunction("deleteContactPhoto") { (id: String, listId: String?) in
      try self.host.deleteContactPhoto(id: id, listId: listId)
    }

    AsyncFunction("createContactListJson") { (name: String) -> String in
      try self.host.createContactListJson(name: name)
    }

    AsyncFunction("deleteContactList") { (id: String) in
      try self.host.deleteContactList(id: id)
    }

    // ─── Collaboration: RSVP (§7.3) + task-list members/sharing (§9.7) ────────
    // Routed Rust-side to the owning external adapter; reads degrade to empty /
    // null for local + unroutable accounts (the UI hides the affordance), writes
    // throw. respondToEvent invalidates the event cache so the next read shows
    // the new status.

    AsyncFunction("calendarCurrentUserEmail") { (calendarId: String) -> String? in
      try self.host.calendarCurrentUserEmail(calendarId: calendarId)
    }

    AsyncFunction("respondToEvent") {
      (calendarId: String, eventId: String, status: String, sendResponse: Bool) in
      try self.host.respondToEvent(
        calendarId: calendarId, eventId: eventId, status: status, sendResponse: sendResponse)
    }

    AsyncFunction("taskListMembersJson") { (listId: String) -> String in
      try self.host.taskListMembersJson(listId: listId)
    }

    AsyncFunction("taskCurrentUserJson") { (listId: String) -> String in
      try self.host.taskCurrentUserJson(listId: listId)
    }

    AsyncFunction("taskListSharesJson") { (listId: String) -> String in
      try self.host.taskListSharesJson(listId: listId)
    }

    AsyncFunction("taskSearchUsersJson") { (listId: String, query: String) -> String in
      try self.host.taskSearchUsersJson(listId: listId, query: query)
    }

    AsyncFunction("taskAddMember") { (listId: String, memberRef: String, right: String?) in
      try self.host.taskAddMember(listId: listId, memberRef: memberRef, right: right)
    }

    AsyncFunction("taskRemoveMember") { (listId: String, memberRef: String) in
      try self.host.taskRemoveMember(listId: listId, memberRef: memberRef)
    }

    AsyncFunction("taskSetMemberRight") { (listId: String, memberRef: String, right: String) in
      try self.host.taskSetMemberRight(listId: listId, memberRef: memberRef, right: right)
    }

    // ─── OAuth (host-driven; mobile opens authorize_url in a native session) ──

    AsyncFunction("beginOauthJson") { (pluginId: String, argsJson: String) -> String in
      try self.host.beginOauthJson(pluginId: pluginId, argsJson: argsJson)
    }

    AsyncFunction("completeOauthJson") { (pluginId: String, requestJson: String) -> String in
      try self.host.completeOauthJson(pluginId: pluginId, requestJson: requestJson)
    }.runOnQueue(slowQueue)

    AsyncFunction("completeOauthReconnectJson") {
      (pluginId: String, accountId: String, requestJson: String) -> String in
      try self.host.completeOauthReconnectJson(
        pluginId: pluginId, accountId: accountId, requestJson: requestJson)
    }.runOnQueue(slowQueue)

    // ─── Discovery (EWS Autodiscover; host-driven, like the desktop) ──────────

    AsyncFunction("discoverJson") { (pluginId: String, argsJson: String) -> String in
      try self.host.discoverJson(pluginId: pluginId, argsJson: argsJson)
    }.runOnQueue(slowQueue)

    // ─── Sync-target OAuth (Dropbox / Google Drive) ───────────────────────────

    AsyncFunction("completeSyncOauthJson") { (pluginId: String, requestJson: String) in
      try self.host.completeSyncOauthJson(pluginId: pluginId, requestJson: requestJson)
    }.runOnQueue(slowQueue)

    // ─── E2E sync encryption (§19.7) ──────────────────────────────────────────

    AsyncFunction("enableSyncEncryptionJson") { (passphrase: String) -> String in
      try self.host.enableSyncEncryptionJson(passphrase: passphrase)
    }.runOnQueue(slowQueue)

    AsyncFunction("disableSyncEncryptionJson") { (passphrase: String) -> String in
      try self.host.disableSyncEncryptionJson(passphrase: passphrase)
    }.runOnQueue(slowQueue)

    AsyncFunction("changeSyncPassphraseJson") { (oldPassphrase: String, newPassphrase: String) in
      try self.host.changeSyncPassphraseJson(
        oldPassphrase: oldPassphrase, newPassphrase: newPassphrase)
    }.runOnQueue(slowQueue)

    AsyncFunction("adoptRemoteEncryptionJson") { (passphrase: String) in
      try self.host.adoptRemoteEncryptionJson(passphrase: passphrase)
    }.runOnQueue(slowQueue)

    // ─── Onboarding: preview + join an existing dataset (§19.11) ──────────────

    AsyncFunction("previewSyncTargetJson") { (configJson: String) -> String in
      try self.host.previewSyncTargetJson(configJson: configJson)
    }.runOnQueue(slowQueue)

    AsyncFunction("acceptRemoteDatasetJson") { (configJson: String, deviceName: String?, passphrase: String?) -> String in
      try self.host.acceptRemoteDatasetJson(
        configJson: configJson, deviceName: deviceName, passphrase: passphrase)
    }.runOnQueue(slowQueue)

    // adoptLocalDatasetJson initialises a FRESH dataset (the unified Connect
    // button's empty-target path), optionally enabling E2E at creation.
    AsyncFunction("adoptLocalDatasetJson") { (configJson: String, deviceName: String?, passphrase: String?) -> String in
      try self.host.adoptLocalDatasetJson(
        configJson: configJson, deviceName: deviceName, passphrase: passphrase)
    }.runOnQueue(slowQueue)

    AsyncFunction("resumeStaleDeviceJson") { () -> String in
      try self.host.resumeStaleDeviceJson()
    }.runOnQueue(slowQueue)

    // ─── SFTP host-key trust (§19.5 TOFU) ─────────────────────────────────────

    AsyncFunction("previewSftpHostKeyJson") { (argsJson: String) -> String in
      try self.host.previewSftpHostKeyJson(argsJson: argsJson)
    }.runOnQueue(slowQueue)

    AsyncFunction("trustSftpHostKey") { (hostPort: String, fingerprint: String) in
      try self.host.trustSftpHostKey(hostPort: hostPort, fingerprint: fingerprint)
    }

    AsyncFunction("forgetSftpHostKey") { (hostPort: String) in
      try self.host.forgetSftpHostKey(hostPort: hostPort)
    }

    AsyncFunction("pinnedSftpHostKey") { (hostPort: String) -> String? in
      try self.host.pinnedSftpHostKey(hostPort: hostPort)
    }
  }
}
