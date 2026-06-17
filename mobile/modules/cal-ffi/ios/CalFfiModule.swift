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
  }
}
