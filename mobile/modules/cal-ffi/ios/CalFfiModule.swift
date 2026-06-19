import ExpoModulesCore
import Foundation

// Calls the Rust `cal_ffi::parse_attendee` through the UniFFI-generated Swift
// bindings (cal_ffi.swift, compiled into this module) backed by
// CalFfi.xcframework. Engine reuse: the same cal-core parser the desktop and
// the Android build use. Mirrors the Android CalFfiModule.
public class CalFfiModule: Module {
  // The full on-device engine: accounts + the statically-embedded adapter
  // registry, opened lazily at the app-sandbox database path. Credentials
  // route through IosKeychain (Security-framework Keychain). Mirrors the
  // Android module's `host`.
  private lazy var host: Host = {
    let dir = try! FileManager.default.url(
      for: .applicationSupportDirectory,
      in: .userDomainMask,
      appropriateFor: nil,
      create: true
    )
    let dbPath = dir.appendingPathComponent("aperio.sqlite").path
    return try! Host.open(dbPath: dbPath, keychain: IosKeychain())
  }()

  public func definition() -> ModuleDefinition {
    Name("CalFfi")

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

    AsyncFunction("deleteAccount") { (accountId: String) in
      try self.host.deleteAccount(accountId: accountId)
    }

    AsyncFunction("renameAccountJson") { (id: String, newName: String) -> String in
      try self.host.renameAccountJson(id: id, newName: newName)
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

    AsyncFunction("getEventByIdJson") { (id: String) -> String in
      try self.host.getEventByIdJson(id: id)
    }

    AsyncFunction("createEventJson") { (requestJson: String) -> String in
      try self.host.createEventJson(requestJson: requestJson)
    }

    AsyncFunction("updateEventJson") { (eventJson: String, previousCalendarId: String?) -> String in
      try self.host.updateEventJson(eventJson: eventJson, previousCalendarId: previousCalendarId)
    }

    AsyncFunction("deleteEvent") { (id: String, calendarId: String?, sendCancellations: Bool?) in
      try self.host.deleteEvent(id: id, calendarId: calendarId, sendCancellations: sendCancellations)
    }

    AsyncFunction("addEventExdateJson") { (id: String, occurrence: String, calendarId: String?) in
      try self.host.addEventExdateJson(id: id, occurrence: occurrence, calendarId: calendarId)
    }

    // ─── Sync ───

    AsyncFunction("configureSyncAdapterJson") { (configJson: String) in
      try self.host.configureSyncAdapterJson(configJson: configJson)
    }

    AsyncFunction("syncStatusJson") { () -> String in
      try self.host.syncStatusJson()
    }

    AsyncFunction("syncNowJson") { () -> String in
      try self.host.syncNowJson()
    }

    AsyncFunction("pushNow") { () -> Int in
      Int(try self.host.pushNow())
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

    AsyncFunction("createContactListJson") { (name: String) -> String in
      try self.host.createContactListJson(name: name)
    }

    AsyncFunction("deleteContactList") { (id: String) in
      try self.host.deleteContactList(id: id)
    }

    // ─── OAuth (host-driven; mobile opens authorize_url in a native session) ──

    AsyncFunction("beginOauthJson") { (pluginId: String, argsJson: String) -> String in
      try self.host.beginOauthJson(pluginId: pluginId, argsJson: argsJson)
    }

    AsyncFunction("completeOauthJson") { (pluginId: String, requestJson: String) -> String in
      try self.host.completeOauthJson(pluginId: pluginId, requestJson: requestJson)
    }

    // ─── Discovery (EWS Autodiscover; host-driven, like the desktop) ──────────

    AsyncFunction("discoverJson") { (pluginId: String, argsJson: String) -> String in
      try self.host.discoverJson(pluginId: pluginId, argsJson: argsJson)
    }

    // ─── Sync-target OAuth (Dropbox / Google Drive) ───────────────────────────

    AsyncFunction("completeSyncOauthJson") { (pluginId: String, requestJson: String) in
      try self.host.completeSyncOauthJson(pluginId: pluginId, requestJson: requestJson)
    }

    // ─── E2E sync encryption (§19.7) ──────────────────────────────────────────

    AsyncFunction("enableSyncEncryptionJson") { (passphrase: String) -> String in
      try self.host.enableSyncEncryptionJson(passphrase: passphrase)
    }

    AsyncFunction("disableSyncEncryptionJson") { (passphrase: String) -> String in
      try self.host.disableSyncEncryptionJson(passphrase: passphrase)
    }

    AsyncFunction("changeSyncPassphraseJson") { (oldPassphrase: String, newPassphrase: String) in
      try self.host.changeSyncPassphraseJson(
        oldPassphrase: oldPassphrase, newPassphrase: newPassphrase)
    }

    AsyncFunction("adoptRemoteEncryptionJson") { (passphrase: String) in
      try self.host.adoptRemoteEncryptionJson(passphrase: passphrase)
    }

    // ─── Onboarding: preview + join an existing dataset (§19.11) ──────────────

    AsyncFunction("previewSyncTargetJson") { (configJson: String) -> String in
      try self.host.previewSyncTargetJson(configJson: configJson)
    }

    AsyncFunction("acceptRemoteDatasetJson") { (configJson: String, deviceName: String?, passphrase: String?) -> String in
      try self.host.acceptRemoteDatasetJson(
        configJson: configJson, deviceName: deviceName, passphrase: passphrase)
    }

    AsyncFunction("resumeStaleDeviceJson") { () -> String in
      try self.host.resumeStaleDeviceJson()
    }

    // ─── SFTP host-key trust (§19.5 TOFU) ─────────────────────────────────────

    AsyncFunction("previewSftpHostKeyJson") { (argsJson: String) -> String in
      try self.host.previewSftpHostKeyJson(argsJson: argsJson)
    }

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
