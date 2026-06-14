---
title: "Aperio — Developer Documentation"
---

Aperio is an accessible, cross-platform calendar and task planner. It aims
to be **fully usable with a screen reader** without ever leaving the
application/focus mode, while remaining a fast, native desktop app.

This book is for **contributors to the application itself**. If you instead
want to write a *plugin* (a new calendar/task/contact provider, a sync
backend, or a video-conferencing integration), read the
[Plugin Developer Documentation](/aperio/plugin-dev/).

## The stack at a glance

- **Backend / host:** Rust, packaged as a [Tauri 2](https://tauri.app)
  application (`src-tauri`, crate name `aperio`). It owns the SQLite
  database, the sync engine, the plugin host, and exposes everything to the
  UI through Tauri commands.
- **Frontend:** React + TypeScript built with Vite (`src/`). It is a
  thin, accessible view layer — all persistence and provider logic live in
  the backend.
- **Core domain + adapters:** a Cargo workspace of ~40 crates under
  `crates/` — `cal-core` (the shared domain types and traits) plus one
  crate per provider adapter and per plugin.

## How to read this book

| Chapter | What you'll find |
|---|---|
| [Getting Started](/developers/getting-started/) | Prerequisites, cloning, running a dev build, the commands you'll use daily. |
| [Architecture](/developers/architecture/) | How the host, frontend, and plugin system fit together; the data and sync paths. |
| [Contributing](/developers/contributing/) | Branching, commits, the accessibility-as-a-gate rule, code style. |
| [Testing](/developers/testing/) | Unit and adapter tests, the accessibility test matrix, CI. |
| [Adapters](/developers/adapters/overview/) | Per-provider notes: protocol, auth, quirks, how to test. |

The authoritative design spec is `DESIGN.md` at the repository root. This
book distills the parts a contributor needs day to day; when in doubt,
`DESIGN.md` wins.
