export const meta = {
  name: 'overrides-extraction-understand',
  description: 'Map the OverridesRepo host-core extraction + read-time stamping for the mobile cal-ffi Host',
  phases: [{ title: 'Understand' }],
}
const AREA_SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    summary: { type: 'string', description: 'How this area works, concrete' },
    keyApis: { type: 'array', items: { type: 'string' }, description: 'Exact fn/type/method names + signatures that matter' },
    files: { type: 'array', items: { type: 'string' }, description: 'Relevant file paths (repo-relative) with line ranges where useful' },
    gotchas: { type: 'array', items: { type: 'string' }, description: 'Pitfalls / coupling / Tauri deps / DB-handle / schema concerns' },
    recommendation: { type: 'string', description: 'Concrete recommendation for the extraction + cal-ffi wiring' },
  },
  required: ['summary', 'keyApis', 'files', 'gotchas', 'recommendation'],
}
const BASE = [
  'Repo C:\\scripts\\aperio. GOAL: extract the desktop OverridesRepo (host-local colour + name overrides for EXTERNAL containers / sections / events, plus local contact lists) into a Tauri-free shared crate (host-core) so the mobile cal-ffi Host can reuse it, then wire it into the mobile Host: external branches for set_container_color_label / rename_container / (new) set_section_color / set_event_color, AND read-time stamping so list_calendars_json / task_lists_json / get_sections / get_events_json apply the overrides (resolve_color_hex + name overrides) exactly as the desktop read paths do.',
  'Context: the mobile Host (crates/cal-ffi/src/host.rs) already implements the LOCAL branches of set_container_color_label + rename_container (local entity carries the binding on its own synced row + emits a sync event). EXTERNAL + contact-list colour/name currently return Unsupported, pending THIS extraction. The desktop OverridesRepo lives at src-tauri/src/overrides.rs and the read-time stamping at src-tauri/src/commands/calendars.rs (+ wherever sections/events get stamped). host-core is crates/host-core (already a Tauri-free shared crate driven by desktop + cal-ffi). The shared SQLite schema/migrations live in crates/aperio-db.',
  'Be concrete (exact names, signatures, file:line, SQL table names). Read the actual code. The output guides a multi-commit Rust refactor.',
].join('\n')
phase('Understand')
const AREAS = [
  { key: 'overrides-repo', prompt: BASE + '\n\nAREA: the OverridesRepo itself. Read src-tauri/src/overrides.rs FULLY. Map: every public type (OverridesRepo, ContainerKind, the override row structs) + every method (set_color_label/clear_color_label/list + set_section_color_label/clear + set_event_color_label/clear + any NAME-override methods + the resolve/apply helpers) with signatures. What does it depend on (rusqlite Connection? a DbHandle? the aperio-db schema? any Tauri/src-tauri-only types)? Which SQLite tables does it own + are those tables defined in aperio-db migrations or created by OverridesRepo? Is it ALREADY Tauri-free (pure rusqlite) or does it import src-tauri types? What exactly must move to host-core and what (if anything) stays desktop-side.' },
  { key: 'read-stamping', prompt: BASE + '\n\nAREA: read-time stamping. Read src-tauri/src/commands/calendars.rs (and any sibling commands for tasks/sections/contacts) to find where overrides are APPLIED on read: resolve_color_hex / apply_color_to_calendar / apply_color_to_event / apply_color_to_task / name-override stamping. Map the exact apply functions + their signatures + WHERE in the read path they run (after fetching calendars/events/sections, before returning). Note the colour resolution PRECEDENCE (override vs native vs label) and how a NAME override replaces the container name on read. This is what cal-ffi list_*_json / get_events_json must replicate.' },
  { key: 'cal-ffi-host', prompt: BASE + '\n\nAREA: the mobile Host wiring points. Read crates/cal-ffi/src/host.rs — the existing set_container_color_label + rename_container methods (their LOCAL branches + the external_*_unsupported helpers), the Host struct fields (does it hold a DbHandle / shared rusqlite handle it can build an OverridesRepo from? how does it call AccountsRepo::new(&shared) — i.e. the shared() accessor), and the read methods that must stamp: list_calendars_json, task_lists_json, get_sections (the sections JSON method), get_events_json. Map exactly how to (a) instantiate OverridesRepo from the Host, (b) add the external branches, (c) stamp on read. Note any is_local_* helpers + the registry route map.' },
  { key: 'host-core-and-desktop', prompt: BASE + '\n\nAREA: host-core placement + desktop re-pointing. Read crates/host-core/src/lib.rs + its module list + Cargo.toml, and how the desktop (src-tauri) currently imports OverridesRepo. Recommend: which host-core module the OverridesRepo should live in (or a new module), what its Cargo deps need (rusqlite, aperio-db?), and how the desktop src-tauri/src/overrides.rs becomes a re-export shim (like other host-core extractions) so desktop commands keep working unchanged. Check crates/aperio-db for whether the override tables are in the shared migrations (they MUST be, so both desktop + cal-ffi DBs have them) or whether they need adding to the shared schema.' },
]
const results = await parallel(
  AREAS.map((a) => () =>
    agent(a.prompt, { label: 'understand:' + a.key, phase: 'Understand', schema: AREA_SCHEMA })
      .then((r) => ({ area: a.key, ...r }))),
)
return { areas: results.filter(Boolean) }
