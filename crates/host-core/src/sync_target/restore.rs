//! What each host does at start-up to get its sync target back.
//!
//! Two readers can answer, and [`super::build_for_device`] decides between them:
//! the account row this device points at, or — only when it points at nothing —
//! the `sync.adapter.*` preferences it has not moved off yet.
//! [`super::migrate_to_account`] turns the second into the first, once.
//!
//! Ordering those is three lines of code and about a dozen of reasoning, and the
//! reasoning is the same on both hosts — which is the whole argument for it
//! living here rather than twice. The desktop and the mobile host contribute
//! what only they can: how to reach a plugin, where their keychain is, where
//! their host-key pins are. When the migration runs, which reader answers, and
//! what the log says are decided once.
//!
//! ## The migration runs FIRST, on the same launch
//!
//! Not after the restore, and not on the launch after it. The account reader has
//! nothing to find on a device that has not migrated, so running the migration
//! afterwards would spend every upgrade launch on the old path for no reason —
//! one launch in which a fault in the new path stays invisible, on the one day
//! someone would still recognise what changed.
//!
//! ## The fallback is deliberate, and it is narrow
//!
//! It fires when this device points at NO account. That is belt and braces for
//! one release: a device whose migration could not run — a database that would
//! not answer for one launch, a keychain that was locked, a stored kind this
//! build has no table for — must keep syncing exactly the way it did yesterday,
//! and the old preferences are still there and still complete precisely so that
//! it can. The migration is retried on the next launch, and the release that
//! deletes the old preferences deletes this fallback with them.
//!
//! A pointer at an account that is GONE is not that case and does not fall back.
//! It is a target the user deleted, and rebuilding the legacy one would resume
//! uploading to the place they moved off — silently, because it would look like
//! an ordinary restore. It is reported instead:
//! [`Unbuildable::AccountMissing`] says so, and the line below names the id.
//! Every other refusal — an untrusted host key, a credential that is gone, a
//! plugin that is not installed — is a device that DOES point at an account and
//! something about it is wrong, and papering over it with the old target would
//! hide exactly the fault the user needs to see.
//!
//! ## One line, so there is something to ask for
//!
//! Every path here logs under one target with one account id, so "what does the
//! sync restore line say" is a question with an answer, on either host.

use std::sync::Arc;

use sync_core::SyncAdapter;
use sync_engine::SecretStore;

use super::build::{build_for_device, Unbuildable};
use super::from_account::{selected_account_id_result, SyncPlugins};
use super::migrate::migrate_to_account;
use crate::accounts::AccountsRepo;
use crate::registry::HostKeyPins;
use crate::user_prefs::UserPrefsRepo;

/// One target for every line this module writes, so a user asked for "the sync
/// restore line" has one thing to search for and a support answer does not
/// depend on which host they are on.
const TARGET: &str = "aperio::sync_target";

/// Migrate if this device still needs it, then restore what it syncs through.
///
/// Returns the adapter the orchestrator should be configured with, already
/// wrapped for encryption where this device encrypts — the readers do that
/// themselves, so no host has to remember to, and neither may wrap it again.
///
/// `None` means nothing is configured, or that what is configured could not be
/// opened. The reason is logged rather than returned because both hosts call
/// this while starting up, where there is no user to hand an error to; the
/// difference from before is that there is now a line to find.
pub fn restore_sync_target(
    prefs: &UserPrefsRepo<'_>,
    accounts: &AccountsRepo<'_>,
    secrets: &dyn SecretStore,
    pins: &dyn HostKeyPins,
    plugins: &dyn SyncPlugins,
) -> Option<Arc<dyn SyncAdapter>> {
    migrate_once(prefs, accounts, secrets);

    // Read for the LOG rather than for the decision — [`build_for_device`]
    // makes the same read and is the one that chooses, so there is still
    // exactly one place where the pointer settles which record answers.
    //
    // Through the FALLIBLE twin, and a read that failed stops the launch here.
    // The infallible one answers "this device has not chosen" for a database
    // that would not say, and downstream that is the answer that opens the
    // legacy preferences — so a single unanswered read could put a device that
    // HAD chosen back on the target it moved off, which is the whole thing this
    // path exists to prevent. Refusing costs one launch of sync and is retried
    // on the next.
    let chosen = match selected_account_id_result(prefs) {
        Ok(chosen) => chosen,
        Err(err) => {
            tracing::warn!(
                target: TARGET,
                %err,
                "could not read which account this device syncs through; it does not \
                 sync this launch rather than risk falling back to a target it was \
                 moved off, and tries again on the next one",
            );
            return None;
        }
    };

    match build_for_device(prefs, accounts, secrets, pins, plugins) {
        Ok(adapter) => {
            match chosen.as_deref() {
                Some(account_id) => tracing::info!(
                    target: TARGET,
                    %account_id,
                    "restored this device's sync target through the account it points at",
                ),
                None => tracing::info!(
                    target: TARGET,
                    "restored this device's sync target from this device's legacy sync \
                     preferences — it points at no account, so the migration has not \
                     reached it",
                ),
            }
            Some(adapter)
        }
        // Neither record has anything. A device that never configured a target
        // is the ordinary case and says so quietly.
        Err(Unbuildable::NotConfigured) => {
            tracing::debug!(target: TARGET, "this device syncs through nothing");
            None
        }
        // The one refusal worth its own line: see the module doc. Reported, not
        // recovered from, and said in the words of what the user would see —
        // they disconnected or deleted an account, and the device did not
        // quietly go back to the target before it.
        Err(Unbuildable::AccountMissing { account_id }) => {
            tracing::warn!(
                target: TARGET,
                %account_id,
                "this device points at a sync account that no longer exists; it does \
                 NOT fall back to the target it was moved off, and does not sync until \
                 it is pointed at an account that exists",
            );
            None
        }
        Err(err) => {
            match chosen.as_deref() {
                Some(account_id) => tracing::warn!(
                    target: TARGET,
                    %account_id,
                    %err,
                    "could not restore this device's sync target through the account it \
                     points at; this device does not sync until that is fixed",
                ),
                None => tracing::warn!(
                    target: TARGET,
                    %err,
                    "could not restore this device's sync target from this device's \
                     legacy sync preferences; this device does not sync until that is \
                     fixed",
                ),
            }
            None
        }
    }
}

/// Run the migration, and say what it did.
///
/// A failure is logged and stepped over rather than propagated: everything the
/// old path needs is still in place, the fallback below will find it, and the
/// migration marker is unwritten so the next launch tries again.
fn migrate_once(prefs: &UserPrefsRepo<'_>, accounts: &AccountsRepo<'_>, secrets: &dyn SecretStore) {
    match migrate_to_account(prefs, accounts, secrets) {
        Ok(Some(account_id)) => tracing::info!(
            target: TARGET,
            %account_id,
            "migrated this device's sync target into an account",
        ),
        // Nothing configured, or this already ran. Both are the steady state
        // and neither is worth a line on every launch.
        Ok(None) => {}
        Err(err) => tracing::warn!(
            target: TARGET,
            %err,
            "could not migrate this device's sync target into an account; this device \
             keeps syncing the way it did before, and the next launch tries again",
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use plugin_core::account_schema::AccountSchema;
    use plugin_core::manifest::PluginManifest;

    use super::*;
    use crate::db::DbHandle;
    use crate::sync_target::persist::FakeSecrets;
    use crate::sync_target::{
        selected_account_id, PLUGIN_ID_WEBDAV, PREF_ADAPTER_KIND, PREF_MIGRATED_ACCOUNT,
        PREF_WEBDAV_URL,
    };

    /// The id the account path opens with, because it is the id this host
    /// RESOLVES for the kind. The legacy path never asks — it opens
    /// [`PLUGIN_ID_WEBDAV`] straight off the table — so the recorded id is
    /// direct evidence of which reader answered, which is what every assertion
    /// below is about.
    const RESOLVED: &str = "resolved-through-the-account";

    /// Records what was opened and refuses to open it. No test here needs a
    /// real adapter: the two builders' own tests already cover what they hand
    /// over, and these are about WHICH one ran.
    #[derive(Default)]
    struct Host {
        opened: Mutex<Vec<String>>,
    }

    impl Host {
        /// The shipped WebDAV schema, so the account path resolves the way it
        /// does on a real device rather than against a hand-written twin.
        fn schema() -> AccountSchema {
            PluginManifest::from_bytes(include_bytes!(
                "../../../sync-adapter-webdav-plugin/plugin.json"
            ))
            .expect("the shipped manifest parses")
            .account
            .expect("it declares an account schema")
        }

        fn opened(&self) -> Vec<String> {
            self.opened.lock().unwrap().clone()
        }
    }

    impl SyncPlugins for Host {
        fn resolve(&self, adapter_kind: &str) -> Option<(String, AccountSchema)> {
            (adapter_kind == "webdav").then(|| (RESOLVED.to_string(), Self::schema()))
        }
        fn open(
            &self,
            plugin_id: &str,
            _config_json: String,
        ) -> Result<Arc<dyn SyncAdapter>, String> {
            self.opened.lock().unwrap().push(plugin_id.to_string());
            Err("captured".into())
        }
    }

    struct NoPins;
    impl HostKeyPins for NoPins {
        fn peek(&self, _host_port: &str) -> Option<String> {
            None
        }
    }

    /// A target as the old code left it: the kind and its fields.
    fn legacy_webdav(prefs: &UserPrefsRepo<'_>) {
        prefs.set(PREF_ADAPTER_KIND, "webdav").unwrap();
        prefs
            .set(PREF_WEBDAV_URL, "https://cloud.example.test/dav/anna/")
            .unwrap();
    }

    /// The upgrade launch: the migration runs and the account reader answers on
    /// the SAME launch, not the one after it.
    #[test]
    fn a_device_that_has_not_migrated_migrates_and_then_restores_through_the_account() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let secrets = FakeSecrets::default();
        legacy_webdav(&prefs);

        let host = Host::default();
        let _ = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &host);

        assert_eq!(
            host.opened(),
            vec![RESOLVED.to_string()],
            "the preference reader answered on a device that had just migrated",
        );
        let id = selected_account_id(&prefs).expect("the migration pointed this device somewhere");
        assert!(accounts.get(&id).unwrap().is_some());
    }

    /// The second launch: no migration to run, and the same reader answers.
    #[test]
    fn an_already_migrated_device_restores_through_the_account() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let secrets = FakeSecrets::default();
        legacy_webdav(&prefs);

        let first = Host::default();
        let _ = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &first);
        let before = accounts.list().unwrap().len();

        let second = Host::default();
        let _ = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &second);
        assert_eq!(second.opened(), vec![RESOLVED.to_string()]);
        assert_eq!(
            accounts.list().unwrap().len(),
            before,
            "the second launch migrated again",
        );
    }

    /// The belt and braces, and the ONLY case that falls back: a device with no
    /// pointer. The marker written without one is what an interrupted run
    /// leaves, and such a device must still come up syncing — through the
    /// preferences it has always had.
    #[test]
    fn a_device_that_points_at_no_account_still_syncs_through_its_preferences() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let secrets = FakeSecrets::default();
        legacy_webdav(&prefs);
        prefs.set(PREF_MIGRATED_ACCOUNT, "gone").unwrap();

        let host = Host::default();
        let _ = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &host);

        assert_eq!(
            host.opened(),
            vec![PLUGIN_ID_WEBDAV.to_string()],
            "the device stopped syncing instead of falling back to its preferences",
        );
    }

    /// The regression this whole arrangement exists for: a pointer at an
    /// account that has been DELETED, with the old preferences still on disk.
    /// Falling back would put the user straight back on the target they moved
    /// off — so nothing is opened at all.
    #[test]
    fn a_pointer_at_a_deleted_account_does_not_resurrect_the_legacy_target() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let secrets = FakeSecrets::default();
        legacy_webdav(&prefs);

        let first = Host::default();
        let _ = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &first);
        let id = selected_account_id(&prefs).expect("migrated");
        // The account is gone; the pointer and the legacy preferences are not.
        accounts.delete(&id).unwrap();

        let host = Host::default();
        let adapter = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &host);
        assert!(adapter.is_none());
        assert!(
            host.opened().is_empty(),
            "a deleted account fell back to the target the user moved off",
        );
        assert_eq!(
            selected_account_id(&prefs).as_deref(),
            Some(id.as_str()),
            "the restore path rewrote this device's choice",
        );
    }

    /// A device that points at an account it cannot OPEN does not quietly sync
    /// through the old preferences either. The refusal is the answer.
    #[test]
    fn a_broken_account_does_not_fall_back() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let secrets = FakeSecrets::default();
        legacy_webdav(&prefs);

        // Migrate first, so the pointer exists…
        let first = Host::default();
        let _ = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &first);

        // …then restore on a host whose plugin for the kind is gone.
        #[derive(Default)]
        struct NoPlugin(Host);
        impl SyncPlugins for NoPlugin {
            fn resolve(&self, _kind: &str) -> Option<(String, AccountSchema)> {
                None
            }
            fn open(&self, plugin_id: &str, cfg: String) -> Result<Arc<dyn SyncAdapter>, String> {
                self.0.open(plugin_id, cfg)
            }
        }
        let host = NoPlugin::default();
        let adapter = restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &host);
        assert!(adapter.is_none());
        assert!(
            host.0.opened().is_empty(),
            "a broken account silently fell back to the old target",
        );
    }

    /// A device that never configured anything gets no adapter, no account and
    /// no fault.
    #[test]
    fn nothing_configured_restores_nothing() {
        let db = DbHandle::open_in_memory().unwrap();
        let shared = db.shared();
        let prefs = UserPrefsRepo::new(&shared);
        let accounts = AccountsRepo::new(&shared);
        let secrets = FakeSecrets::default();

        // The built-in local store is an account too, so this is "no NEW
        // account", which is what the migration must not create out of nothing.
        let before = accounts.list().unwrap().len();
        let host = Host::default();
        assert!(restore_sync_target(&prefs, &accounts, &secrets, &NoPins, &host).is_none());
        assert!(host.opened().is_empty());
        assert_eq!(accounts.list().unwrap().len(), before);
        assert_eq!(prefs.get(PREF_MIGRATED_ACCOUNT).unwrap(), None);
    }
}
