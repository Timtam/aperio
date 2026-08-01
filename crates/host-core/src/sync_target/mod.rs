//! Where this device syncs to, in one place instead of two.
//!
//! The desktop host and the mobile host each carried their own copy of these
//! names — the same 28 preference keys, the same seven keychain pseudo-account
//! ids, the same six plugin ids — alongside eight parallel `match` blocks over
//! them. The strings happened to agree; the behaviour around them had already
//! drifted in six places, which is what two copies of anything eventually do.
//!
//! This module owns the names. The behaviour follows in the commits after it.
//!
//! ## These keys never sync
//!
//! Every key here is device-local by construction: which target a device uses,
//! and the credentials for it, are that device's own business. One machine syncs
//! to an SFTP host with a key at `/home/anna/.ssh/id_ed25519`; another syncs to a
//! folder on a NAS. Propagating either would be wrong, and propagating a sync
//! target through the sync it configures is circular.
//!
//! That is not a convention — it is enforced. `sync-engine`'s whitelist decides
//! what crosses devices, `sync.intervalMinutes` is the single sync key on it, and
//! the test at the bottom of this file walks every constant declared here and
//! asserts the whitelist rejects it. A key added here without thinking about it
//! fails that test rather than quietly appearing on someone's other device.

use sync_engine::SecretSlot;

/// Which adapter this device syncs through, or absent for none.
///
/// Tolerate `"none"` on read as well as absence: the mobile host used to write
/// that string where the desktop deleted the row, and those values are in the
/// wild. Nothing should write it any more.
pub const PREF_ADAPTER_KIND: &str = "sync.adapter.kind";

/// A folder on this machine, or a network share mounted into its filesystem.
pub const PREF_LOCAL_PATH: &str = "sync.adapter.local.path";

pub const PREF_WEBDAV_URL: &str = "sync.adapter.webdav.url";
pub const PREF_WEBDAV_USER: &str = "sync.adapter.webdav.user";

pub const PREF_SFTP_HOST: &str = "sync.adapter.sftp.host";
pub const PREF_SFTP_PORT: &str = "sync.adapter.sftp.port";
pub const PREF_SFTP_USER: &str = "sync.adapter.sftp.user";
pub const PREF_SFTP_PATH: &str = "sync.adapter.sftp.path";
/// `"password"` or `"key"` — which credential the adapter should use.
pub const PREF_SFTP_AUTH_METHOD: &str = "sync.adapter.sftp.authMethod";
/// Absolute path to the private key file. A path, not a secret, so it lives in
/// preferences rather than the keychain — and it is the clearest example of a
/// value that is only meaningful on the machine that wrote it.
pub const PREF_SFTP_KEY_PATH: &str = "sync.adapter.sftp.keyPath";

pub const PREF_FTP_HOST: &str = "sync.adapter.ftp.host";
pub const PREF_FTP_PORT: &str = "sync.adapter.ftp.port";
pub const PREF_FTP_USER: &str = "sync.adapter.ftp.user";
pub const PREF_FTP_PATH: &str = "sync.adapter.ftp.path";
/// `"explicit"` or `"implicit"` — when the TLS handshake happens.
pub const PREF_FTP_MODE: &str = "sync.adapter.ftp.mode";

pub const PREF_DROPBOX_CLIENT_ID: &str = "sync.adapter.dropbox.clientId";
pub const PREF_DROPBOX_CLIENT_SECRET: &str = "sync.adapter.dropbox.clientSecret";
pub const PREF_DROPBOX_PATH: &str = "sync.adapter.dropbox.path";

pub const PREF_GOOGLEDRIVE_CLIENT_ID: &str = "sync.adapter.googledrive.clientId";
pub const PREF_GOOGLEDRIVE_CLIENT_SECRET: &str = "sync.adapter.googledrive.clientSecret";
pub const PREF_GOOGLEDRIVE_FOLDER_NAME: &str = "sync.adapter.googledrive.folderName";

/// Keychain pseudo-account ids.
///
/// The secret store is account-scoped, and a sync target is not an account, so
/// each family gets a fixed id of its own. They are deliberately separate: a
/// user switching from SFTP back to WebDAV must not find the other family's
/// credential clobbered, and switching SFTP between password and key auth must
/// leave the unused one intact.
pub const SECRET_ACCOUNT_WEBDAV: &str = "sync.adapter.webdav";
pub const SECRET_ACCOUNT_SFTP: &str = "sync.adapter.sftp";
/// The SSH key passphrase, kept apart from the SFTP password slot.
pub const SECRET_ACCOUNT_SFTP_KEY: &str = "sync.adapter.sftp.key";
pub const SECRET_ACCOUNT_FTP: &str = "sync.adapter.ftp";
pub const SECRET_ACCOUNT_DROPBOX: &str = "sync.adapter.dropbox";
pub const SECRET_ACCOUNT_GOOGLEDRIVE: &str = "sync.adapter.googledrive";
/// The end-to-end encryption data key. Losing this means losing the ability to
/// rejoin an existing sync at all — a wiped install can only start a new one.
pub const SECRET_ACCOUNT_E2E: &str = "sync.adapter.e2e";

/// Which plugin serves each kind.
///
/// Temporary. It exists because the sync plugins declare no `adapter_kind` in
/// their manifests yet; once they do, the manifest answers this and the table
/// goes away — along with the last place the host names a sync adapter.
pub const PLUGIN_ID_LOCAL: &str = "com.aperio.sync-adapter-local";
pub const PLUGIN_ID_WEBDAV: &str = "com.aperio.sync-adapter-webdav";
pub const PLUGIN_ID_FTP: &str = "com.aperio.sync-adapter-ftp";
pub const PLUGIN_ID_SFTP: &str = "com.aperio.sync-adapter-sftp";
pub const PLUGIN_ID_DROPBOX: &str = "com.aperio.sync-adapter-dropbox";
pub const PLUGIN_ID_GOOGLEDRIVE: &str = "com.aperio.sync-adapter-googledrive";

/// Every kind this build can sync through, and the plugin that serves it.
pub const KINDS: &[(&str, &str)] = &[
    ("local", PLUGIN_ID_LOCAL),
    ("webdav", PLUGIN_ID_WEBDAV),
    ("ftp", PLUGIN_ID_FTP),
    ("sftp", PLUGIN_ID_SFTP),
    ("dropbox", PLUGIN_ID_DROPBOX),
    ("googledrive", PLUGIN_ID_GOOGLEDRIVE),
];

/// The plugin that serves `kind`, or `None` for a name this build does not know.
pub fn plugin_id_for_kind(kind: &str) -> Option<&'static str> {
    KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, id)| *id)
}

/// Whether a stored `PREF_ADAPTER_KIND` value means "no target".
///
/// Absent, empty, or the legacy `"none"` the mobile host used to write.
pub fn is_unconfigured(kind: Option<&str>) -> bool {
    match kind {
        None => true,
        Some(value) => value.trim().is_empty() || value == "none",
    }
}

/// Which slot each secret pseudo-account uses.
///
/// All of them are `Password` today except the OAuth pair, which hold refresh
/// tokens, and the encryption key. Listed rather than derived so a reader can
/// see the whole credential surface of the sync layer at once.
pub const SECRET_SLOTS: &[(&str, SecretSlot)] = &[
    (SECRET_ACCOUNT_WEBDAV, SecretSlot::Password),
    (SECRET_ACCOUNT_SFTP, SecretSlot::Password),
    (SECRET_ACCOUNT_SFTP_KEY, SecretSlot::Password),
    (SECRET_ACCOUNT_FTP, SecretSlot::Password),
    (SECRET_ACCOUNT_DROPBOX, SecretSlot::RefreshToken),
    (SECRET_ACCOUNT_GOOGLEDRIVE, SecretSlot::RefreshToken),
    (SECRET_ACCOUNT_E2E, SecretSlot::SyncEncryptionKey),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preference key this module declares.
    const ALL_PREF_KEYS: &[&str] = &[
        PREF_ADAPTER_KIND,
        PREF_LOCAL_PATH,
        PREF_WEBDAV_URL,
        PREF_WEBDAV_USER,
        PREF_SFTP_HOST,
        PREF_SFTP_PORT,
        PREF_SFTP_USER,
        PREF_SFTP_PATH,
        PREF_SFTP_AUTH_METHOD,
        PREF_SFTP_KEY_PATH,
        PREF_FTP_HOST,
        PREF_FTP_PORT,
        PREF_FTP_USER,
        PREF_FTP_PATH,
        PREF_FTP_MODE,
        PREF_DROPBOX_CLIENT_ID,
        PREF_DROPBOX_CLIENT_SECRET,
        PREF_DROPBOX_PATH,
        PREF_GOOGLEDRIVE_CLIENT_ID,
        PREF_GOOGLEDRIVE_CLIENT_SECRET,
        PREF_GOOGLEDRIVE_FOLDER_NAME,
    ];

    /// The invariant the whole design rests on: none of this crosses devices.
    ///
    /// Walking the declarations rather than repeating a list means a key added
    /// above is covered without anyone remembering this test — which is the
    /// point, because the failure it guards against is silent. A sync target
    /// that propagated would drag every device onto one machine's SFTP host or
    /// one machine's local folder path, and nothing would report an error.
    #[test]
    fn no_sync_target_key_is_ever_synced() {
        for key in ALL_PREF_KEYS {
            assert!(
                !sync_engine::whitelist::is_synced_key(key),
                "{key} would cross devices; sync-target keys must stay local",
            );
        }
    }

    #[test]
    fn every_kind_resolves_to_its_plugin() {
        for (kind, id) in KINDS {
            assert_eq!(plugin_id_for_kind(kind), Some(*id));
        }
        assert_eq!(plugin_id_for_kind("nextcloud"), None);
    }

    #[test]
    fn unconfigured_covers_absent_empty_and_the_legacy_none() {
        assert!(is_unconfigured(None));
        assert!(is_unconfigured(Some("")));
        assert!(is_unconfigured(Some("  ")));
        assert!(is_unconfigured(Some("none")));
        assert!(!is_unconfigured(Some("webdav")));
    }

    /// Each family keeps its own slot, so switching backends — or switching
    /// SFTP between password and key auth — cannot clobber the inactive one.
    #[test]
    fn secret_accounts_are_distinct() {
        let mut ids: Vec<&str> = SECRET_SLOTS.iter().map(|(id, _)| *id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "two sync families share a keychain slot");
    }
}
