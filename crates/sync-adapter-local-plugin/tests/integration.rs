//! Integration test for the local-filesystem sync plugin
//! (ABI v2).
//!
//! Exercises the full FFI pipeline without dlopen:
//!
//!   1. Call the plugin's `build_descriptor` directly.
//!   2. Register the descriptor with plugin-core's
//!      `PluginManager` via `register_static`.
//!   3. `open_instance` against a temp-dir remote_root, get
//!      back a `LoadedInstance`.
//!   4. Wrap in `FfiSyncAdapter`.
//!   5. Drive each `SyncAdapter` trait method + assert
//!      round-trips.
//!
//! ABI v2 lets each test open its OWN instance against its own
//! tempdir — there's no shared singleton state anymore.

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest, shim::FfiSyncAdapter,
    Capability, PluginType, ABI_VERSION,
};
use sync_core::{DeviceCursor, MetaJson, SyncAdapter};
use tempfile::TempDir;

fn plugin_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.sync-adapter-local".into(),
        name: "Aperio Local Filesystem".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::SyncAdapter,
        capabilities: vec![],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
        recurrence: Default::default(),
        tasks: Default::default(),
        account: None,
        adapter_kind: None,
    }
}

/// Build a manager + register the plugin descriptor. Each
/// test gets its own manager so loaded plugins don't collide
/// on the "duplicate id" check.
fn make_manager() -> PluginManager {
    let manager = PluginManager::new("0.1.0");
    let descriptor: *mut AperioPlugin = unsafe { sync_adapter_local_plugin::build_descriptor() };
    assert!(!descriptor.is_null(), "create returned NULL");
    let destroy_fn: unsafe extern "C" fn(*mut AperioPlugin) = sync_adapter_local_plugin::DESTROY_FN;
    manager
        .register_static(plugin_manifest(), descriptor, destroy_fn)
        .expect("register_static");
    manager
}

/// Set up a tempdir, open a fresh instance against its
/// remote_root, and wrap in FfiSyncAdapter. Returns everything
/// the caller needs to keep alive for the duration of the test.
fn setup() -> (TempDir, PluginManager, Arc<FfiSyncAdapter>) {
    let tmp = TempDir::new().expect("tempdir");
    let remote_root = tmp.path().join("aperio-sync");
    std::fs::create_dir_all(&remote_root).expect("mkdir");

    let manager = make_manager();
    let loaded = manager
        .get("com.aperio.sync-adapter-local")
        .expect("registered");
    let cfg = serde_json::json!({
        "remote_root": remote_root.to_string_lossy(),
    });
    let instance = manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open_instance");
    let adapter = FfiSyncAdapter::new(instance).expect("vtable surface");
    (tmp, manager, Arc::new(adapter))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_connection_succeeds_against_a_valid_root() {
    let (_tmp, _mgr, adapter) = setup();
    adapter
        .test_connection()
        .await
        .expect("test_connection should succeed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_meta_returns_none_on_empty_remote() {
    let (_tmp, _mgr, adapter) = setup();
    let meta = adapter.fetch_meta().await.expect("fetch_meta");
    assert!(meta.is_none(), "fresh remote has no meta.json yet");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_then_fetch_meta_round_trips() {
    let (_tmp, _mgr, adapter) = setup();
    let meta = MetaJson::fresh("0.1.0");
    adapter.push_meta(&meta).await.expect("push_meta");
    let echoed = adapter
        .fetch_meta()
        .await
        .expect("fetch_meta")
        .expect("Some after push");
    assert_eq!(echoed.schema_version, meta.schema_version);
    assert_eq!(echoed.min_app_version, meta.min_app_version);
    assert_eq!(echoed.devices.len(), meta.devices.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_new_logs_empty_when_no_logs_pushed() {
    let (_tmp, _mgr, adapter) = setup();
    let logs = adapter
        .fetch_new_logs(&DeviceCursor::epoch())
        .await
        .expect("fetch_new_logs");
    assert!(logs.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_sound_asset_returns_none_for_unknown_hash() {
    let (_tmp, _mgr, adapter) = setup();
    let result = adapter
        .fetch_sound_asset("deadbeef", "mp3")
        .await
        .expect("fetch_sound_asset call");
    assert!(result.is_none(), "no asset pushed yet");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_then_fetch_sound_asset_round_trips_bytes() {
    let (_tmp, _mgr, adapter) = setup();
    let bytes = b"FAKE OGG BYTES".to_vec();
    let hash = "abcdef123456";
    adapter
        .push_sound_asset(hash, "ogg", &bytes)
        .await
        .expect("push_sound_asset");
    let echoed = adapter
        .fetch_sound_asset(hash, "ogg")
        .await
        .expect("fetch_sound_asset")
        .expect("Some after push");
    assert_eq!(echoed, bytes, "byte payload must survive FFI round-trip");
}

/// ABI v2 must support multiple parallel instances against the
/// same loaded library (DESIGN.md §6.4). Each instance writes
/// to its own remote_root and they don't see each other's data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_instances_have_independent_remote_roots() {
    let tmp_a = TempDir::new().expect("tempdir a");
    let tmp_b = TempDir::new().expect("tempdir b");
    let root_a = tmp_a.path().join("aperio-sync");
    let root_b = tmp_b.path().join("aperio-sync");
    std::fs::create_dir_all(&root_a).expect("mkdir a");
    std::fs::create_dir_all(&root_b).expect("mkdir b");

    let manager = make_manager();
    let loaded = manager
        .get("com.aperio.sync-adapter-local")
        .expect("registered");

    let cfg_a = serde_json::json!({ "remote_root": root_a.to_string_lossy() });
    let cfg_b = serde_json::json!({ "remote_root": root_b.to_string_lossy() });
    let inst_a = manager
        .open_instance(loaded.clone(), &cfg_a.to_string())
        .expect("open a");
    let inst_b = manager
        .open_instance(loaded, &cfg_b.to_string())
        .expect("open b");
    assert_ne!(
        inst_a.handle() as usize,
        inst_b.handle() as usize,
        "v2 must hand out one handle per open_instance call",
    );

    let adapter_a = FfiSyncAdapter::new(inst_a).expect("a");
    let adapter_b = FfiSyncAdapter::new(inst_b).expect("b");

    // Push meta to A; B's remote stays empty.
    adapter_a
        .push_meta(&MetaJson::fresh("0.1.0"))
        .await
        .expect("push a");
    let meta_b = adapter_b.fetch_meta().await.expect("fetch b");
    assert!(
        meta_b.is_none(),
        "instance B should NOT see instance A's data"
    );
}

#[test]
fn manifest_declares_no_capabilities_for_sync_adapter() {
    let m = plugin_manifest();
    assert_eq!(m.plugin_type, PluginType::SyncAdapter);
    assert!(m.capabilities.is_empty());
    assert!(!m.has_capability(&Capability::Calendar));
    assert!(!m.has_capability(&Capability::Tasks));
    assert!(!m.has_capability(&Capability::Contacts));
}
