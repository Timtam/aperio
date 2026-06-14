---
title: "Aperio — Plugin Developer Documentation"
---

Aperio is extensible through **plugins**: self-contained shared libraries
the app loads at runtime to add a new data provider or backend, without
recompiling the app.

## What a plugin can be

A plugin declares one or more **capabilities**:

- **calendar-adapter** — a data source for calendars, task lists, and/or
  contacts (any combination). Google, iCloud (CalDAV), Microsoft Graph,
  Vikunja, Todoist, … are all calendar-adapter plugins. A tasks-only
  provider simply declares `["tasks"]`.
- **sync-adapter** — a storage backend for cross-device sync (WebDAV,
  Dropbox, SFTP, …).
- **vc-adapter** — a video-conferencing integration (Meet, Zoom, …).

This book focuses on calendar-adapter plugins (the most common kind); the
ABI shape is the same for the others.

## How it works, briefly

The app (the **host**) talks to a plugin across a stable **C ABI** — a
table of function pointers (a *vtable*) per capability. You don't have to
write raw FFI: the **Rust SDK** (`plugin-sdk`) gives you macros so you
implement ordinary Rust traits and a couple of macros generate the ABI
glue. Domain data crosses the boundary as serde JSON.

Because it's a C ABI, a plugin can in principle be written in any language
that can export C symbols (Rust is recommended; C/C++/Zig are possible),
and the ABI is versioned for forward compatibility.

## Read this in order

| Chapter | What you'll do |
|---|---|
| [Getting Started](/plugins/getting-started/) | Build a working minimal plugin in under 15 minutes. |
| [The C ABI](/plugins/abi-reference/) | The full contract: structs, lifecycle, vtables, memory rules, versioning. |
| [The Rust SDK](/plugins/rust-sdk/) | The macros and helpers that hide the FFI. |
| [The manifest](/plugins/manifest/) | Every `plugin.json` field. |
| [Examples](/plugins/examples/) | `hello-world` and a full calendar-adapter template. |

> The contributor-facing docs for the app itself are the
> [Developer Documentation](/developers/).
