//! Every field of a connect form reaches the frontends whole.
//!
//! The bug this pins was not a wrong value, it was a missing key.
//! `crates/cal-ffi` built the account-form JSON by hand with `serde_json::json!`
//! and left out `options`, `default_bool`, `default_text` and `device_local`,
//! while `mobile/src/components/AccountSchemaForm.tsx` renders a `choice` field
//! with `field.options.map(…)`. The FTP and SFTP plugins both declare a choice
//! field, so adding either account on a phone dereferenced `undefined` and took
//! the form down.
//!
//! Nothing caught it. The desktop built the same spec correctly from its own
//! copy, and the mobile side parses the bridge's answer with a bare
//! `as AccountFormSpec` cast — the TypeScript type said the key was there,
//! because a person had typed it there.
//!
//! There is one builder now (`host_core::account_form`) and the TypeScript is
//! generated from it, so the class of bug is structurally gone. These two tests
//! are what makes that checkable rather than merely believed.

use std::collections::BTreeMap;

use host_core::account_form::spec_from_manifest;
use plugin_core::account_schema::AccountFieldKind;
use plugin_core::manager::PluginManager;
use plugin_core::manifest::PluginManifest;
use plugin_core::strings::StringCatalogue;

fn empty_catalogue() -> StringCatalogue {
    StringCatalogue(BTreeMap::new())
}

/// A manifest declaring one field of each interesting kind.
///
/// Written here rather than borrowed from a plugin on purpose: this test must
/// keep meaning something on a tree where every adapter has moved into its own
/// repository, and a test that reads its subject from a neighbouring crate
/// stops asking anything the day that neighbour leaves.
const MANIFEST_JSON: &str = r#"{
  "id": "com.example.formshapes",
  "name": "Form shapes",
  "version": "0.1.0",
  "plugin_type": "adapter",
  "capabilities": ["sync"],
  "abi_version": 4,
  "min_app_version": "0.1.0",
  "author": "Aperio Contributors",
  "description": "A fixture that declares one field of each interesting kind.",
  "adapter_kind": "formshapes",
  "account": {
    "fields": [
      {
        "key": "transport",
        "kind": "choice",
        "label": "Transport",
        "options": [
          { "value": "explicit", "label": "Explicit" },
          { "value": "implicit", "label": "Implicit" }
        ]
      },
      { "key": "root", "kind": "directory", "label": "Root", "device_local": true },
      { "key": "passive", "kind": "bool", "label": "Passive", "default": true },
      { "key": "host", "kind": "text", "label": "Host", "default": "example.org" }
    ]
  }
}"#;

#[test]
fn every_field_carries_every_key_the_forms_read() {
    let manifest: PluginManifest =
        serde_json::from_str(MANIFEST_JSON).expect("the fixture manifest parses");
    let spec = spec_from_manifest(&manifest, &empty_catalogue(), "en")
        .expect("the fixture declares an account form");
    let json = serde_json::to_value(&spec).expect("the spec serialises");

    let fields = json["fields"].as_array().expect("fields is an array");
    assert_eq!(
        fields.len(),
        4,
        "the fixture's four fields all came through"
    );

    // Named, not counted: these are the four keys the mobile form reads and the
    // hand-built JSON omitted. A field missing any of them is the crash.
    for field in fields {
        for key in [
            "key",
            "kind",
            "label",
            "hint",
            "required",
            "options",
            "default_bool",
            "default_text",
            "device_local",
        ] {
            assert!(
                field.get(key).is_some(),
                "field {:?} has no `{key}` — the mobile form reads it without a guard",
                field["key"],
            );
        }
    }

    let choice = fields
        .iter()
        .find(|f| f["kind"] == "choice")
        .expect("the fixture declares a choice field");
    let options = choice["options"].as_array().expect("options is an array");
    assert_eq!(
        options.len(),
        2,
        "a choice field's options must survive; `AccountSchemaForm` maps over them",
    );
    assert_eq!(options[0]["value"], "explicit");
    assert_eq!(options[0]["label"], "Explicit");

    // The other three shapes, each of which had its own missing key.
    let by_key = |k: &str| {
        fields
            .iter()
            .find(|f| f["key"] == k)
            .unwrap_or_else(|| panic!("the fixture declares {k}"))
    };
    assert_eq!(by_key("root")["device_local"], true);
    assert_eq!(by_key("passive")["default_bool"], true);
    assert_eq!(by_key("host")["default_text"], "example.org");
    // A field that is not a choice publishes an EMPTY list, never a null: the
    // mobile form maps over it unconditionally.
    assert_eq!(
        by_key("host")["options"].as_array().map(|o| o.len()),
        Some(0),
    );
}

/// And the adapters this build actually ships say the same.
///
/// Asked of the registry rather than of `crates/*/plugin.json`, like
/// `manifests_parse.rs`: the set that matters is what this build links, and
/// that does not change when a crate moves into its own repository.
///
/// A build that links no plugin with a choice field gets no assertions here and
/// that is correct — the shape itself is pinned above, where it cannot go
/// vacuous. What this adds is the fact that made the bug reachable rather than
/// theoretical: real shipped adapters declare choice fields.
#[test]
fn the_shipped_adapters_publish_their_choices() {
    let manager = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&manager).expect("every bundled plugin registers");

    let mut with_choices: Vec<String> = Vec::new();
    for plugin in manager.all() {
        let Some(spec) = spec_from_manifest(&plugin.manifest, &empty_catalogue(), "en") else {
            continue;
        };
        let declares_choice = plugin
            .manifest
            .account
            .as_ref()
            .is_some_and(|s| s.fields.iter().any(|f| f.kind == AccountFieldKind::Choice));
        if !declares_choice {
            continue;
        }
        with_choices.push(plugin.manifest.id.clone());
        for field in spec
            .fields
            .iter()
            .filter(|f| f.kind == AccountFieldKind::Choice)
        {
            assert!(
                !field.options.is_empty(),
                "{}'s `{}` is a choice field with no options — the form would render \
                 an empty radio group",
                plugin.manifest.id,
                field.key,
            );
        }
    }

    // Reported rather than asserted on: which adapters this build ships is a
    // build question, and pinning the list here would make adding one a test
    // failure. Printing it keeps the fact visible in the log.
    println!(
        "adapters declaring a choice field: {}",
        if with_choices.is_empty() {
            "(none in this build)".to_string()
        } else {
            with_choices.join(", ")
        },
    );
}
