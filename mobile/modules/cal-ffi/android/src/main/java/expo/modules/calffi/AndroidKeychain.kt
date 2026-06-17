package expo.modules.calffi

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import uniffi.cal_ffi.KeychainBridge
import uniffi.cal_ffi.KeychainException

/**
 * Android implementation of the Rust [KeychainBridge] foreign trait — the
 * platform half of the engine's `SecretStore` seam.
 *
 * The desktop stores account credentials in the OS keyring; on Android we use
 * [EncryptedSharedPreferences] wrapped by a master key held in the
 * AndroidKeyStore (hardware-backed where the device supports it). Credentials
 * therefore never sit in plaintext on disk and never enter the SQLite database
 * — exactly the split the desktop keeps.
 *
 * Keys are `"$accountId:$slot"`; `slot` is the stable wire name the Rust side
 * passes (`"password"`, `"api_token"`, …, from `SecretSlot::wire_name`), so the
 * layout matches what the registry's `register_*` paths read back.
 *
 * A missing entry on [retrieve] throws [KeychainException.NotFound] so the
 * Rust `SecretError::NotFound` distinction (e.g. an absent optional iCal
 * password) survives the round-trip; any other failure becomes
 * [KeychainException.Backend].
 */
class AndroidKeychain(context: Context) : KeychainBridge {
  // `:` separator: account ids are UUIDs and slot names are fixed lowercase
  // ASCII (`SecretSlot::wire_name`), so a colon can't collide with either half.
  private fun key(accountId: String, slot: String): String = "$accountId:$slot"

  private val prefs: SharedPreferences by lazy {
    try {
      val appContext = context.applicationContext
      val masterKey = MasterKey.Builder(appContext)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()
      EncryptedSharedPreferences.create(
        appContext,
        "aperio_secrets",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
      )
    } catch (e: Throwable) {
      // A corrupt keystore / prefs file would otherwise crash on every access.
      // Surface it as a backend error so account ops fail cleanly instead.
      throw KeychainException.Backend("could not open the encrypted store: ${e.message}")
    }
  }

  override fun store(accountId: String, slot: String, value: String) {
    try {
      prefs.edit().putString(key(accountId, slot), value).apply()
    } catch (e: Throwable) {
      throw KeychainException.Backend(e.message ?: "store failed")
    }
  }

  override fun retrieve(accountId: String, slot: String): String {
    val stored = try {
      prefs.getString(key(accountId, slot), null)
    } catch (e: Throwable) {
      throw KeychainException.Backend(e.message ?: "retrieve failed")
    }
    return stored ?: throw KeychainException.NotFound()
  }

  override fun delete(accountId: String, slot: String) {
    try {
      prefs.edit().remove(key(accountId, slot)).apply()
    } catch (e: Throwable) {
      throw KeychainException.Backend(e.message ?: "delete failed")
    }
  }

  override fun deleteAll(accountId: String) {
    try {
      val prefix = "$accountId:"
      val editor = prefs.edit()
      for (existing in prefs.all.keys) {
        if (existing.startsWith(prefix)) editor.remove(existing)
      }
      editor.apply()
    } catch (e: Throwable) {
      throw KeychainException.Backend(e.message ?: "deleteAll failed")
    }
  }
}
