---
title: "Getting Started"
---

This chapter gets a new contributor from a fresh clone to a running
development build.

## Prerequisites

| Tool | Why | Notes |
|---|---|---|
| **Rust** (stable) | Backend, all workspace crates | Install via [rustup](https://rustup.rs). The pinned toolchain is set in the workspace `Cargo.toml` (`rust-version`). |
| **Node.js** (LTS) + npm | Frontend (Vite, React, TypeScript) | Any current LTS works. |
| **Tauri CLI** | Build/run the desktop app | `cargo install tauri-cli` (or use `npm run tauri`). |
| **System WebView** | Tauri's renderer | WebView2 on Windows, WebKitGTK on Linux, WKWebView on macOS. See the [Tauri prerequisites](https://tauri.app/start/prerequisites/). |

## Clone and set up

```sh
git clone https://github.com/Timtam/aperio
cd aperio

# Frontend dependencies
npm install

# Rust dependencies are fetched on first build.
```

## Run a development build

```sh
# Starts Vite + the Tauri shell with hot-reload.
npm run tauri dev
# (equivalent to `cargo tauri dev`)
```

The first build compiles the whole Rust workspace and can take a while;
subsequent builds are incremental.

## Notifications on Windows in a development build

Windows shows a notification only when a Start menu shortcut carries the
notification's app id; the portable app offers that shortcut under
**Settings, General, Windows notifications** (DESIGN.md §14.3, §17.5).

A development build needs none. When the exe lies in a cargo `target\debug`
or `target\release` folder, tauri-plugin-notification sends its
notifications under PowerShell's app id, so they appear as **Windows
PowerShell** without any shortcut. Aperio follows the same rule
(`platform::toasts_use_powershell_id`, a port of the plugin's, case-sensitive
like it): the settings section says it is a development build and offers no
buttons, the hint never opens, and the repair at start changes nothing.

A build in any other `CARGO_TARGET_DIR`, or a cross-compiled
`target\<triple>\release`, counts as a real copy to the plugin. Its
notifications go out as `com.aperio.app` and need the shortcut, and at start
it points your real Start menu entry at itself, because the entry then points
at another copy. Start your everyday copy again, or set the entry up from it,
to point it back.

## Commands you'll use daily

Frontend (`package.json` scripts):

```sh
npm run dev          # Vite dev server only (no Tauri shell)
npx tsc --noEmit     # TypeScript type-check
npm run lint         # ESLint (.ts/.tsx)
npm run test         # Vitest (unit tests)
npm run build        # tsc --noEmit && vite build (what CI runs)
```

Backend / workspace:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # exactly what CI runs
cargo fmt --all -- --check                              # CI's first gate

# After changing a type in cal-core / plugin-core / host-core: regenerate the
# TypeScript declarations the frontends parse. CI checks they are current.
cargo xtask ts-types
cargo xtask ts-types --check                            # what CI runs

# After a chrono-tz update, or a new CLDR windowsZones file in
# crates/cal-core/cldr/: regenerate the zone tables, in this order.
# CI checks both are current.
cargo xtask tz-list
cargo xtask windows-zones
cargo xtask tz-list --check && cargo xtask windows-zones --check   # what CI runs
```

> **Match CI locally.** CI runs `clippy --workspace --all-targets -D
> warnings`, which lints *test* code too and fails on any warning. A plain
> `cargo clippy` skips test targets and can pass while CI fails. Always run
> the full command above (and `cargo fmt --all -- --check`) before pushing.

## Where do I start?

Pick the layer that matches your interest:

| You want to work on… | Start in… |
|---|---|
| Domain types & traits shared by everything | `crates/cal-core` |
| The host: Tauri commands, DB, sync, plugin host | `src-tauri/src` |
| The UI | `src/` (React/TS) |
| A specific provider (Google, iCloud, …) | `crates/adapter-*` — see [Adapters](/developers/adapters/overview/) |
| iCalendar text ↔ events (CalDAV, feeds, file import, Exchange's imported invitations) | `crates/ical-mapping` |
| The sync engine (event log, CRDT-ish merge) | `crates/sync-core`, `src-tauri/src/event_log`, `src-tauri/src/sync` |
| Writing a *new* provider as a plugin | the [Plugin Developer docs](/plugins/) |

The [Architecture](/developers/architecture/) chapter explains how these pieces talk
to each other.
