//! The world zone list's offsets for the desktop.
//!
//! The names, the search and where a stored zone stands come from the core
//! through WebAssembly (`src/wasm/coreRules.ts`). The offsets need chrono-tz,
//! which that module leaves out — it would grow by about 879 KB — while this
//! binary links it already through host-core. The list asks for them when it
//! opens and when the series' start changes, never while it renders, so an IPC
//! round trip costs nothing that matters.

use super::CommandResult;

/// Every listed zone's offset at the series' start and its standard offset as
/// of today, and the order of the list. See `cal_core::zone_offsets`.
#[tauri::command]
pub async fn zone_offsets(
    question: cal_core::ZoneOffsetsQuestion,
) -> CommandResult<cal_core::ZoneOffsets> {
    Ok(cal_core::zone_offsets(&question))
}
