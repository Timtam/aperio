---
title: "Getting Started"
---

Goal: a loadable plugin that the app recognises, in under 15 minutes. It
will implement just enough to list one (empty) calendar.

## 1. Prerequisites

- **Rust** (stable) with the ability to build a `cdylib` (the default
  toolchain can).
- The Aperio source tree, or at least the published `plugin-sdk` and
  `cal-core` crates, available as dependencies.

## 2. Create a cdylib crate

```toml
# Cargo.toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]   # produces a loadable shared library

[dependencies]
cal-core = { path = "../aperio/crates/cal-core" }      # or the published crate
plugin-sdk = { path = "../aperio/crates/plugin-sdk" }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## 3. Implement the adapter trait(s)

Write ordinary Rust. Implement `Adapter` (base) plus the feature traits you
support — here just a minimal `CalendarFeature` that returns no calendars:

```rust
use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability,
    Credentials, Result,
};

pub struct MyAdapter;

#[async_trait]
impl Adapter for MyAdapter {
    async fn authenticate(&self, _c: Credentials) -> Result<AuthToken> {
        Ok(AuthToken::default())
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Calendar]
    }
}

#[async_trait]
impl CalendarFeature for MyAdapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        Ok(vec![]) // a real plugin would call its provider here
    }
    // ... other CalendarFeature methods (see the template example)
}
```

## 4. Export the ABI glue

The SDK macros turn your trait impl into the C ABI the host loads. You
declare the lifecycle and the vtable; see [The Rust SDK](/plugins/rust-sdk/) for
the full pattern (`declare_lifecycle!`, the per-method `ffi_*` wrappers
generated with `cal_dispatch_helpers!`, and the static `*_VTABLE`).

## 5. Write `plugin.json`

```json
{
  "id": "com.example.my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "plugin_type": "calendar-adapter",
  "capabilities": ["calendars"],
  "abi_version": 2,
  "min_app_version": "0.1.0",
  "author": "You",
  "description": "A minimal example calendar adapter.",
  "signed": false
}
```

See [the manifest reference](/plugins/manifest/) for every field.

## 6. Build, package, install

```sh
cargo build --release   # produces target/release/{lib}my_plugin.{so,dll,dylib}
```

Package the shared library together with `plugin.json` into a `.aperio`
archive, then install it from the app's plugin settings and try it. The
[`hello-world` example](/plugins/examples/hello-world/) is exactly this, complete.

## Where to go next

- Don't want to hand-write each `ffi_*` wrapper? That's what the
  [Rust SDK](/plugins/rust-sdk/) macros are for.
- Building a *real* calendar adapter? Start from the
  [calendar-adapter-template](/plugins/examples/calendar-adapter-template/).
- Curious what crosses the boundary and why it stays compatible? Read
  [The C ABI](/plugins/abi-reference/).
