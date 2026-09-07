//! "Verbindung testen" against a real SFTP plugin, through the real registry.
//!
//! This test exists because of the seam it sits on. `host-core` owns
//! `probe_account` but cannot depend on a sync adapter, and
//! `adapter-sftp-plugin` owns the refusal but knows nothing about probing — so
//! the one combination that mattered was invisible to both test suites:
//!
//!   the plugin refuses an unpinned host  +  the probe never merged the pin
//!   = the test button reports a fault on an account that is pinned and syncing.
//!
//! This crate links both, which makes it the only place the question can be
//! asked. It is asked with the SHIPPED plugin rather than a stand-in, because a
//! hand-written twin would have agreed with whatever the host did.

#![cfg(feature = "sftp")]

use std::sync::Arc;

use host_core::accounts::AdapterKind;
use host_core::registry::{AdapterRegistry, HostKeyPins, RegistryError};
use plugin_core::manager::PluginManager;

const SFTP_ID: &str = "com.aperio.sync-adapter-sftp";

/// A pin store that answers the same thing for every host.
struct Pins(Option<String>);
impl HostKeyPins for Pins {
    fn peek(&self, _host_port: &str) -> Option<String> {
        self.0.clone()
    }
}

fn registry_with_sftp() -> AdapterRegistry {
    let manager = Arc::new(PluginManager::new("0.1.0"));
    host_plugins::register_all_static(&manager).expect("register the bundled plugins");
    assert!(manager.get(SFTP_ID).is_some(), "SFTP plugin is linked in");
    AdapterRegistry::new(
        manager,
        Arc::new(sync_engine::test_support::FakeSecrets::default()),
    )
}

/// The config the accounts form produces: every declared field, and no pin —
/// `pinned_fingerprint` is not a schema field, it is merged by the host.
fn form_values() -> String {
    serde_json::json!({
        "host": "backup.example.invalid",
        "port": 22,
        "user": "alice",
        "path": "/home/alice/aperio",
        "auth_method": "password",
    })
    .to_string()
}

#[tokio::test]
async fn testing_an_unconfirmed_sftp_host_says_the_host_key_is_not_trusted() {
    // Not "adapter construction failed", and not the plugin's own English
    // sentence: `HostKeyNotTrusted` is the one both frontends already translate
    // and pair with a "check the fingerprint" button.
    let registry = registry_with_sftp();
    registry.set_host_key_pins(Arc::new(Pins(None)));

    let err = registry
        .probe_account(&AdapterKind::new("sftp"), &form_values(), Some("swordfish"))
        .await
        .expect_err("an unconfirmed host must not be probed");

    match err {
        RegistryError::HostKeyNotTrusted { host_port } => {
            assert_eq!(host_port, "backup.example.invalid:22");
        }
        other => panic!("the user cannot act on this: {other}"),
    }
}

#[tokio::test]
async fn testing_a_confirmed_sftp_host_passes() {
    // The regression this file was written for. SFTP declares only `sync`, so
    // the probe has no listing to run and opening the instance IS the whole
    // test — which means a missing pin merge turns "Verbindung testen" into a
    // permanent failure on an account whose sync works perfectly.
    let registry = registry_with_sftp();
    registry.set_host_key_pins(Arc::new(Pins(Some("SHA256:abcd1234".into()))));

    registry
        .probe_account(&AdapterKind::new("sftp"), &form_values(), Some("swordfish"))
        .await
        .expect("a pinned host is exactly the account the button must call fine");
}
