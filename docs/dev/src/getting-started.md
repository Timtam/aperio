# Getting Started

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
| A specific provider (Google, iCloud, …) | `crates/cal-adapter-*` — see [Adapters](adapters/overview.md) |
| The sync engine (event log, CRDT-ish merge) | `crates/sync-core`, `src-tauri/src/event_log`, `src-tauri/src/sync` |
| Writing a *new* provider as a plugin | the [Plugin Developer book](/aperio/plugin-dev/) |

The [Architecture](architecture.md) chapter explains how these pieces talk
to each other.
