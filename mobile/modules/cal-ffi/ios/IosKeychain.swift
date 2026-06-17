import Foundation
import Security

/// iOS implementation of the Rust `KeychainBridge` foreign trait — the platform
/// half of the engine's `SecretStore` seam (the desktop uses the OS keyring;
/// Android uses Keystore-backed EncryptedSharedPreferences).
///
/// Each secret is a `kSecClassGenericPassword` item keyed by
/// `service = "Aperio:<slot>"`, `account = <accountId>`, mirroring the desktop
/// keyring's service/user split. Items use
/// `kSecAttrAccessibleAfterFirstUnlock` (NOT `WhenUnlocked`): a background sync
/// round must be able to read the refresh token while the device is locked, so
/// long as it has been unlocked once since boot.
///
/// A missing item on `retrieve` throws `KeychainError.NotFound` so the Rust
/// `SecretError::NotFound` distinction (e.g. an absent optional iCal password)
/// survives the round-trip; any other OSStatus becomes `KeychainError.Backend`.
final class IosKeychain: KeychainBridge {
  private func service(_ slot: String) -> String { "Aperio:\(slot)" }

  private func baseQuery(_ accountId: String, _ slot: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service(slot),
      kSecAttrAccount as String: accountId,
    ]
  }

  func store(accountId: String, slot: String, value: String) throws {
    guard let data = value.data(using: .utf8) else {
      throw KeychainError.Backend(detail: "could not encode secret as UTF-8")
    }
    // Delete-then-add is the simplest overwrite that also (re)sets the
    // accessibility attribute deterministically.
    SecItemDelete(baseQuery(accountId, slot) as CFDictionary)
    var add = baseQuery(accountId, slot)
    add[kSecValueData as String] = data
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
    let status = SecItemAdd(add as CFDictionary, nil)
    guard status == errSecSuccess else {
      throw KeychainError.Backend(detail: "SecItemAdd failed: \(status)")
    }
  }

  func retrieve(accountId: String, slot: String) throws -> String {
    var query = baseQuery(accountId, slot)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound {
      throw KeychainError.NotFound
    }
    guard status == errSecSuccess else {
      throw KeychainError.Backend(detail: "SecItemCopyMatching failed: \(status)")
    }
    guard let data = item as? Data, let value = String(data: data, encoding: .utf8) else {
      throw KeychainError.Backend(detail: "stored secret was not valid UTF-8")
    }
    return value
  }

  func delete(accountId: String, slot: String) throws {
    let status = SecItemDelete(baseQuery(accountId, slot) as CFDictionary)
    // A missing item is success — `delete` is best-effort/idempotent.
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw KeychainError.Backend(detail: "SecItemDelete failed: \(status)")
    }
  }

  func deleteAll(accountId: String) throws {
    // No service filter: clear every slot for this account in one pass.
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrAccount as String: accountId,
    ]
    let status = SecItemDelete(query as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw KeychainError.Backend(detail: "SecItemDelete(all) failed: \(status)")
    }
  }
}
