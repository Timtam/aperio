---
title: "Getting Started"
---

Goal: a loadable plugin that the app recognises, in under 15 minutes. It
will implement just enough to list one (empty) calendar.

## 1. Prerequisites

- **Rust** (stable) with the ability to build a `cdylib` (the default
  toolchain can).
- The Aperio source tree, or at least the `plugin-sdk` crate, available as a
  dependency. That is the only Aperio crate you name: it re-exports
  `cal_core`, `sync_core`, `vc_core` and `plugin_core`, so everything you need
  is reachable through it. One name is one version to keep in step, which is
  what you want when your plugin lives in its own repository.

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
plugin-sdk = { path = "../aperio/crates/plugin-sdk" }  # the only Aperio crate
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## 3. Implement the adapter trait(s)

Write ordinary Rust. Implement `Adapter` (base) plus the feature traits you
support — here just a minimal `CalendarFeature` that returns no calendars:

```rust
use async_trait::async_trait;
use plugin_sdk::cal_core::{
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
  "plugin_type": "adapter",
  "capabilities": ["calendar"],
  "abi_version": 4,
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

A `.aperio` archive is a **zip file**, flat, with two things in it:

```
plugin.json                       your manifest, verbatim
com.example.my-plugin.dll         your library, named after your plugin id
```

The library's extension is the platform's — `.dll`, `.dylib`, `.so` — and its
stem is your plugin id, because that is the first name the host looks for. Any
other files you put in are extracted alongside and ignored.

```sh
cd target/release
cp ../../plugin.json .
cp libmy_plugin.so com.example.my-plugin.so
zip com.example.my-plugin-1.0.0-x86_64-unknown-linux-gnu.aperio     plugin.json com.example.my-plugin.so
```

**One archive per platform.** Nothing inside says which one it is: the host
picks the library by file extension, so a Windows arm64 build and a Windows x64
build produce archives that look identical and are not interchangeable. Put the
target triple in the filename — it is the only place a person can see it.

Then install it from Settings → Plugins and try it. The
[`hello-world` example](/plugins/examples/hello-world/) is exactly this, complete.

> Aperio's own bundled adapters are packed the same way, by
> `cargo xtask pack-plugins` in its repository. Whatever breaks about the format
> breaks for them first.

## Where to go next

- Don't want to hand-write each `ffi_*` wrapper? That's what the
  [Rust SDK](/plugins/rust-sdk/) macros are for.
- Building a *real* calendar adapter? Start from the
  [calendar adapter template](/plugins/examples/calendar-adapter-template/).
- Curious what crosses the boundary and why it stays compatible? Read
  [The C ABI](/plugins/abi-reference/).
