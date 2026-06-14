# Aperio

An accessible, keyboard-first calendar, task, and contact application for
the desktop — built screen-reader-first, local-first, and provider-agnostic.

> **Status: in active development, pre-release.** The core feature set is
> implemented and in daily use, but there are **no binary releases yet** —
> you currently build from source. The remaining gaps are tracked openly in
> [`TODO.md`](TODO.md); the full specification lives in
> [`DESIGN.md`](DESIGN.md) (German).

## What it does

- **Calendars, tasks, and contacts as equals** — multiple sources side by
  side, each with its own color and visibility toggle.
- **Providers:** Google, iCloud and any CalDAV/CardDAV server,
  Outlook / Microsoft 365 (Graph), Exchange (EWS), read-only iCal feeds
  (e.g. public holidays), Vikunja, and Todoist — plus purely local
  calendars and lists that need no account at all.
- **Views:** day, week, month, year, agenda, and a dedicated task view —
  including a backlog column in the week/month planners for
  drag-and-drop day planning (tasks *and* events).
- **The usual suspects, done accessibly:** recurring events with
  occurrence/series handling, reminders with per-item notification sounds,
  free/busy lookups, RSVP, color labels, full-text search, system-tray
  background mode.
- **Cross-device sync over *your* storage** — WebDAV, Dropbox, Google
  Drive, SFTP, FTP, or a shared folder. There is no Aperio server; an
  append-only event log with snapshots keeps devices in step. Optional
  **end-to-end encryption** (AES-256-GCM, Argon2id key derivation) covers
  the whole dataset — with E2E enabled, even account credentials sync
  encrypted so accounts work on every device without re-entry.
- **Plugin architecture:** every provider adapter is a plugin behind a
  stable C ABI, so additional backends can be written in any language and
  installed as `.aperio` packages at runtime.
- **Languages:** English and German UI and documentation.

## Accessibility

Accessibility is the project's founding constraint, not an afterthought:

- Developed and manually tested **screen-reader-first** (primarily NVDA);
  dialogs keep focus mode stable, views expose proper grid/list/tree
  semantics, and every state change is announced via live regions.
- **Keyboard-first:** every action is reachable without a mouse
  (Outlook-style two-level navigation, `F6` region cycling, documented
  shortcuts). Drag-and-drop is always a *redundant* mouse affordance on
  top of an existing keyboard path.

## Documentation

The documentation is one Astro Starlight site (landing page, legal pages and
all docs), published via GitHub Pages: **<https://timtam.github.io/aperio/>**
The source lives in [`web/`](web/).

| Section | Audience |
|---|---|
| [User Guide (English)](https://timtam.github.io/aperio/guides/) | Using Aperio |
| [Benutzerhandbuch (Deutsch)](https://timtam.github.io/aperio/de/guides/) | Aperio benutzen |
| [Developer Guide](https://timtam.github.io/aperio/developers/) | Architecture, contributing |
| [Plugin Development](https://timtam.github.io/aperio/plugins/) | Writing adapters against the C ABI |

In-repo: [`DESIGN.md`](DESIGN.md) is the complete (German) specification
the implementation is audited against; [`TODO.md`](TODO.md) is the
code-verified list of what is still open.

> **Connecting Google (for now):** Aperio does not ship a verified Google
> app registration yet, so connecting a Google account currently requires
> your own free Google Cloud OAuth client. The user guide contains a
> [step-by-step walkthrough](https://timtam.github.io/aperio/guides/google-oauth/).
> An official, published registration is planned — afterwards this step
> disappears.

## Building from source

| Tool       | Version                                                       |
|------------|---------------------------------------------------------------|
| Rust       | ≥ 1.80                                                        |
| Node.js    | ≥ 20                                                          |
| Tauri CLI  | 2.x (`npm install` installs it as a dev dependency)           |

Platform-specific prerequisites: see the
[Tauri docs](https://v2.tauri.app/start/prerequisites/).

### Linux

For Tauri builds:

```bash
sudo apt-get install \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libssl-dev \
  libgtk-3-dev \
  libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev
```

### Windows

[WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) ships
with Windows 10 21H2+ and Windows 11. On older systems the app downloads
a bootstrap installer on first start.

### Development

```bash
# Install frontend dependencies
npm install

# Dev mode (backend + frontend with hot reload)
npm run tauri dev

# Tests
cargo test --workspace
npm test

# Lint
cargo clippy --workspace --all-targets -- -D warnings
npm run lint
```

Aperio runs **portable** by default: if a writable `data/` directory exists
(or can be created) next to the binary, all data lives there; otherwise it
falls back to the platform's user-profile directory.

## Project layout

```
aperio/
├── Cargo.toml              # Workspace root
├── DESIGN.md               # Full specification (German)
├── TODO.md                 # Open implementation gaps (code-verified)
├── crates/
│   ├── cal-core/           # Shared calendar/task/contact types and traits
│   ├── sync-core/          # Cross-device sync: event log, snapshots, E2E crypto
│   ├── vc-core/            # Video-conferencing trait surface
│   ├── plugin-core/        # Plugin C ABI + manager
│   ├── plugin-sdk/         # Rust SDK for plugin authors
│   ├── cal-adapter-*/      # Calendar/task/contact adapters (+ -plugin crates)
│   ├── sync-adapter-*/     # Sync-storage adapters (+ -plugin crates)
│   └── vc-adapter-*/       # Video-conferencing adapters (stubs, + -plugin crates)
├── web/                    # Astro Starlight site: landing + docs (user de/en, dev, plugin)
├── src-tauri/              # Tauri backend (commands, sync engine, reminders)
└── src/                    # React/TypeScript frontend
```

## On the use of AI

Aperio is developed in close collaboration with an AI coding assistant
(Anthropic's Claude, driven through Claude Code): a large share of the
code, tests, and documentation is AI-written under continuous human
direction. Design decisions, feature priorities, and acceptance are made
by a human; changes are verified by the full test/lint suite, security- and
correctness-critical work additionally goes through adversarial AI review
passes, and accessibility behaviour is tested manually with a screen
reader. If you find that something slipped through regardless — issues and
bug reports are very welcome.

## License

MIT OR Apache-2.0.
