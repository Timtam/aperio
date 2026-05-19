# Aperio

An accessible, cross-platform calendar and task application.

> **Status:** Phase 0 (project skeleton) — app functionality is not yet
> implemented. The full specification lives in [`DESIGN.md`](DESIGN.md)
> (in German).

## Core principles

- **Accessibility first** — full compatibility with NVDA, JAWS, Narrator, VoiceOver, Orca
- **Keyboard-first** — every action reachable without a mouse (Outlook-style two-level navigation)
- **Calendars and tasks as equals** — separate data models, independent sync logic
- **Lightweight and portable** — single executable per platform
- **Cross-platform** — Windows, macOS, Linux (Tauri 2)

## Prerequisites

| Tool       | Version                                                       |
|------------|---------------------------------------------------------------|
| Rust       | ≥ 1.80                                                        |
| Node.js    | ≥ 20                                                          |
| Tauri CLI  | 2.x (`npm install` installs it as a dev dependency)           |

Platform-specific prerequisites: see the [Tauri docs](https://v2.tauri.app/start/prerequisites/).

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

## Development

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

## Project layout

```
aperio/
├── Cargo.toml              # Workspace root
├── crates/                 # Core libraries and adapters
│   ├── cal-core/           # Shared types and traits
│   ├── plugin-core/        # Plugin ABI (Phase 8)
│   ├── plugin-sdk/         # SDK for plugin authors (Phase 8)
│   ├── sync-core/          # Event-log types (Phase 7)
│   ├── cal-adapter-*/      # Data adapters (Phase 6)
│   ├── sync-adapter-*/     # Sync adapters (Phase 7)
│   └── vc-adapter-*/       # Video-conferencing adapters (Phase 8)
├── src-tauri/              # Tauri backend
└── src/                    # React/TypeScript frontend
```

The target architecture is documented in [`DESIGN.md`](DESIGN.md) section 23.

## License

MIT OR Apache-2.0.
