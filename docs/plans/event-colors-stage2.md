# Stage 2 — Native per-event colors (RFC 7986 COLOR for color-capable CalDAV)

Follow-up to the merged Stage 1 (`feat/event-colors`), which made per-event
colors a **host-local override** for external calendars so coloring an iCloud /
CalDAV event no longer triggers a provider PUT (no "network error", color
persists). Stage 2 adds **native** color round-trip where the provider supports
it, falling back to the Stage 1 override where it doesn't.

## Goal

- Color-capable CalDAV servers (non-iCloud, RFC 7986 COLOR): write/read the
  color natively so it syncs to other clients and round-trips.
- iCloud, Microsoft Graph, EWS, iCal feeds: keep the Stage 1 host-local
  override.
- Local: unchanged (color on the event row).

## Settled design decisions

- **`supports_event_color: bool` on `cal_core::Calendar`** (`#[serde(default)]`).
  Adapters set it: **Local = true**, **CalDAV = !iCloud** (set in the CalDAV
  adapter's `lib.rs` after listing, via `server_url.contains("icloud.com")`),
  **Google / Graph / EWS / iCal = false**.
- **`color_hex: Option<String>` on `Event` AND `NewEvent`**
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`). Transport-only
  field: the host resolves `color_label` (id) → hex before a write; an adapter
  fills it from the provider's `COLOR` on read; the host maps it back to a
  `color_label`.
- Both new fields ride over the plugin FFI via **serde** → **no vtable / ABI
  change**.
- **Single source of truth per calendar**: color-capable → native on the
  provider (no override row); non-capable → host-local override (Stage 1).
- **COLOR value format**: v1 writes `#RRGGBB` (accepted by most servers that
  honor COLOR). Read accepts hex and maps known CSS3 color names → hex via a
  small lookup table (ignore otherwise).
- **Read-back without ad-hoc labels**: on read, match `color_hex` only against
  **existing** color labels (by hex). Colors Aperio itself wrote round-trip
  (their label still exists); foreign colors with no matching label resolve to
  no label (acceptable for v1).

## Phases

### 2a — Model plumbing (cal-core + ~40 construction sites)

- Add the two fields to `crates/cal-core/src/types.rs`
  (`Calendar.supports_event_color`, `Event.color_hex`, `NewEvent.color_hex`).
- Fix **every** construction site (the cal-core change breaks the whole
  workspace). Known sites (from the WIP build sweep), all mechanical:
  - `supports_event_color`: local calendars → `true`; CalDAV/Graph/Google/EWS/
    iCal/birthday → `false`. Files: `cal-adapter-local/src/{calendars,sync_snapshot}.rs`,
    `cal-adapter-caldav/src/calendars.rs`, `cal-adapter-microsoft-graph/src/mapping.rs`,
    `cal-adapter-google/src/mapping.rs`, `cal-adapter-ews/src/mapping.rs`,
    `cal-adapter-ical/src/lib.rs`, `plugin-core/src/shim/calendar.rs` (test fixtures),
    `src-tauri/src/commands/birthdays.rs`.
  - `color_hex`: all `Event` / `NewEvent` read constructions → `None`; the CalDAV
    write path (`mapping.rs::event_to_ical`) and the host cross-calendar-move
    `NewEvent` (`src-tauri/src/commands/calendars.rs`) → carry
    `event.color_hex.clone()`. Many test fixtures across `cal-adapter-{local,
    caldav,ews,google,microsoft-graph}` need `color_hex: None`.
- Verify: `cargo build --workspace` + `cargo clippy --workspace --all-targets`
  compile (iterate on the missing-field errors until clean).
- Commit as a single "model plumbing" commit (behavior unchanged: CalDAV
  `supports_event_color` still false here → identical to Stage 1).

### 2b — CalDAV native COLOR (read + write)

- **iCloud detection**: in `cal-adapter-caldav/src/lib.rs` `list_calendars`,
  post-process each returned calendar:
  `cal.supports_event_color = !self.<config>.server_url.contains("icloud.com")`.
- **Write**: in `mapping.rs::apply_common` (the VEVENT builder), emit a
  `COLOR:<hex>` line when `new.color_hex` is `Some` — gated so iCloud never
  receives it (pass the iCloud/capable flag into the builder, or clear
  `color_hex` for iCloud at the `events.rs` create/update boundary).
- **Read**: in `mapping.rs::map_event`, parse the `COLOR` iCalendar property →
  `color_hex` (accept `#RRGGBB`; map known CSS3 names → hex; else leave `None`).
- Tests (mockito): write an event with `color_hex` set → assert `COLOR:` in the
  PUT body; map a VEVENT carrying `COLOR:` → assert `color_hex`. Add a
  CSS3-name → hex unit test for the lookup.

### 2c — Host: hex ↔ label resolution + routing

- Palette helpers (in `overrides.rs` or a small `color_labels` helper):
  `resolve_label_to_hex(id) -> Option<String>` and
  `match_hex_to_label(hex) -> Option<String>` (queries on `color_labels`).
- `update_event` / `create_event` (`src-tauri/src/commands/calendars.rs`): for a
  color-capable external calendar (account != LOCAL and the calendar's
  `supports_event_color`), resolve `event.color_label` → `color_hex` before
  calling the adapter, so the adapter writes `COLOR`. (Look up the calendar's
  capability via the cache/registry, or thread it through.)
- `get_events`: **no capability lookup needed** — first map any event with
  `color_hex` set (⇔ a color-capable provider) → `color_label` via
  `match_hex_to_label`; then run the existing `apply_color_to_events` override
  stamp for the rest (non-capable externals, which never carry `color_hex`).
- `set_event_color`: extend the early `Ok(())` no-op to also cover color-capable
  external calendars (so a stray call can't create an override that competes
  with the native value).

### 2d — Frontend: route color by capability

- `src/api/types.ts` `Calendar`: add `supports_event_color?: boolean` (it rides
  the wire already via the `CalendarRow` serde-flatten; just type it).
- `src/state/useChipContextMenu.ts` (event color submenu) and
  `src/components/EventDialog.tsx`: route the color by
  `account_id === 'local' || calendar.supports_event_color` → `update_event`
  (native); else → `setEventColor` (override). Stop calling `setEventColor` for
  color-capable external calendars.

### 2e — Tests, docs, review

- cal-core / host / adapter tests (incl. the round-trip and hex↔label match).
- User docs (`docs/user` + `docs/user-en`, identical trees): per-event colors —
  recolor via right-click / the event dialog; native on color-capable CalDAV,
  local elsewhere. Keep `mdbook build` clean.
- DESIGN.md: the per-event-color capability + the native-vs-override split.
- Adversarial review over the diff (model plumbing completeness, the iCloud
  gate, the hex↔label round-trip, the frontend routing).

## Risks / notes

- **Churn order**: add the cal-core fields first; the workspace won't build
  until every construction site is fixed. Do 2a as one focused, compile-clean
  commit.
- **iCloud heuristic** is URL-based. A generic CalDAV server that *ignores*
  COLOR (rather than rejecting it) would drop the color on the next sync — rare,
  not iCloud, and user-reportable.
- **No new DB migration**: Stage 2 is model + logic only; the Stage 1
  `event_color_overrides` table (migration 0026) stays for non-capable
  providers.
- **Verification parity**: `cargo clippy --workspace --all-targets -- -D warnings`
  + `cargo test` + `cargo fmt`; `tsc` + `eslint` + `vitest`; `mdbook build` ×2.
